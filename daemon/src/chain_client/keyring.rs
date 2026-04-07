use alloy::primitives::{
    Address as EthAddress,
    B256,
};
use alloy::signers::SignerSync;
use alloy::signers::local::{
    MnemonicBuilder,
    PrivateKeySigner,
};
use bip39::Mnemonic;
use sha2::{
    Digest,
    Sha256,
};
use subxt_signer::sr25519::Keypair;
use subxt_signer::{
    DeriveJunction,
    ExposeSecret,
    SecretString,
};
use thiserror::Error;
use tokio::sync::{
    mpsc,
    oneshot,
};
use tracing::instrument;
use zeroize::{
    Zeroize,
    ZeroizeOnDrop,
};

use super::asset_hub::{
    AssetHubAccountId,
    AssetHubSignedTransaction,
    AssetHubUnsignedTransaction,
};
use super::polygon::{
    PAYMASTER,
    PolygonSignedTransaction,
    PolygonUnsignedTransaction,
    SignedPermit,
    UserOperationParams,
};

#[cfg_attr(test, mockall_double::double)]
pub use client::KeyringClient;

const KEYRING_CHANNEL_CAPACITY: usize = 32;

pub type DerivationParams = Vec<String>;

pub struct SignTransactionRequestData<T> {
    pub transaction: T,
    pub derivation_params: DerivationParams,
}

pub struct GenerateAddressData {
    pub derivation_params: DerivationParams,
}

impl From<DerivationParams> for GenerateAddressData {
    fn from(derivation_params: DerivationParams) -> Self {
        Self {
            derivation_params,
        }
    }
}

pub struct SignPermitRequestData {
    pub permit_hash: B256,
    pub derivation_params: DerivationParams,
}

#[derive(Debug, PartialEq, Eq, Clone, Copy, Error)]
pub enum KeyringError {
    #[error("Seed is invalid")]
    InvalidSeed,
    #[error("Unexpected error while send request to or receive response from Keyring")]
    MessageTransmissionFailed,
    #[expect(dead_code)]
    #[error("Timeout while send request to Keyring")]
    ResponseTimeout,
    #[error("Transaction signing failed")]
    SigningFailed,
}

pub type KeyringResult<T> = Result<T, KeyringError>;

#[derive(Zeroize, ZeroizeOnDrop)]
pub struct Keyring {
    /// Seed phrase used for both Asset Hub (sr25519) and Polygon (secp256k1)
    seed: SecretString,
}

impl Keyring {
    pub fn new(seed: SecretString) -> Self {
        Self {
            seed,
        }
    }

    pub fn ignite(
        self
    ) -> (
        tokio::task::JoinHandle<()>,
        KeyringClient,
    ) {
        let (tx, mut rx) = mpsc::channel(KEYRING_CHANNEL_CAPACITY);

        let handle = tokio::spawn(async move {
            while let Some(msg) = rx.recv().await {
                self.process_message(msg);
            }

            tracing::info!("Keyring actor has been shut down");
        });

        let client = KeyringClient::new(tx);

        (handle, client)
    }

    // ========================================================================
    // Asset Hub (sr25519) Key Derivation
    // ========================================================================

    fn generate_asset_hub_derived_keypair(
        &self,
        params: DerivationParams,
    ) -> KeyringResult<Keypair> {
        let mut mnemonic =
            Mnemonic::parse(self.seed.expose_secret()).map_err(|_| KeyringError::InvalidSeed)?;

        let keypair =
            Keypair::from_phrase(&mnemonic, None).map_err(|_| KeyringError::InvalidSeed)?;

        mnemonic.zeroize();

        let derived_keypair = keypair.derive(
            params
                .into_iter()
                .map(DeriveJunction::hard),
        );

        Ok(derived_keypair)
    }

    fn process_sign_asset_hub_transaction(
        &self,
        data: SignTransactionRequestData<AssetHubUnsignedTransaction>,
    ) -> KeyringResult<AssetHubSignedTransaction> {
        let SignTransactionRequestData {
            mut transaction,
            derivation_params,
        } = data;
        let derived_keypair = self.generate_asset_hub_derived_keypair(derivation_params)?;
        Ok((*transaction).sign(&derived_keypair).into())
    }

    fn process_generate_asset_hub_address(
        &self,
        data: GenerateAddressData,
    ) -> KeyringResult<AssetHubAccountId> {
        let derived_keypair = self.generate_asset_hub_derived_keypair(data.derivation_params)?;
        Ok(derived_keypair
            .public_key()
            .to_account_id())
    }

    // ========================================================================
    // Polygon (secp256k1) Key Derivation
    // ========================================================================

    fn generate_polygon_derived_signer(
        &self,
        params: DerivationParams,
    ) -> KeyringResult<PrivateKeySigner> {
        let mnemonic_str = self.seed.expose_secret();

        let mut hasher = Sha256::new();
        for param in params {
            hasher.update(param.as_bytes());
        }
        let hash = hasher.finalize();

        let account = u32::from_be_bytes([hash[0], hash[1], hash[2], hash[3]]) & 0x7FFFFFFF;
        let index = u32::from_be_bytes([hash[4], hash[5], hash[6], hash[7]]);

        // Use both account and index for collision avoid
        let path = format!("m/44'/60'/{account}'/0/{index}");

        // Use alloy's MnemonicBuilder to create a signer with derivation path
        let signer = MnemonicBuilder::<alloy::signers::local::coins_bip39::English>::default()
            .phrase(mnemonic_str)
            .derivation_path(&path)
            .map_err(|e| {
                tracing::debug!(
                    error.category = crate::utils::logging::category::CHAIN_CLIENT,
                    error.source = ?e,
                    path = %path,
                    "Invalid derivation path"
                );
                KeyringError::InvalidSeed
            })?
            .build()
            .map_err(|e| {
                tracing::debug!(
                    error.category = crate::utils::logging::category::CHAIN_CLIENT,
                    error.source = ?e,
                    path = %path,
                    "Failed to derive Polygon signer from mnemonic"
                );
                KeyringError::InvalidSeed
            })?;

        Ok(signer)
    }

    fn process_sign_polygon_permit(
        &self,
        data: SignPermitRequestData,
    ) -> KeyringResult<SignedPermit> {
        let SignPermitRequestData {
            permit_hash,
            derivation_params,
        } = data;

        let signer = self.generate_polygon_derived_signer(derivation_params)?;
        let signature = signer
            .sign_hash_sync(&permit_hash)
            .unwrap();

        Ok(SignedPermit {
            signature,
        })
    }

    fn process_sign_polygon_transaction(
        &self,
        data: SignTransactionRequestData<PolygonUnsignedTransaction>,
    ) -> KeyringResult<PolygonSignedTransaction> {
        let SignTransactionRequestData {
            transaction,
            derivation_params,
        } = data;

        let Some(paymaster_data) = transaction.paymaster_data.clone() else {
            return Err(KeyringError::SigningFailed)
        };

        let Some(op_hash) = transaction.op_hash else {
            return Err(KeyringError::SigningFailed)
        };

        let signer = self.generate_polygon_derived_signer(derivation_params)?;

        let authorization = transaction.authorization.clone();
        let auth_signature = signer
            .sign_hash_sync(&authorization.signature_hash())
            .unwrap();
        let eip7702_auth = authorization
            .into_signed(auth_signature)
            .into();

        let signature = signer
            .sign_hash_sync(&op_hash)
            .unwrap()
            .as_bytes();

        let op_params = UserOperationParams {
            sender: transaction.sender,
            nonce: transaction.entrypoint_nonce,
            call_data: const_hex::encode_prefixed(&transaction.call_data),
            paymaster: PAYMASTER,
            paymaster_data,
            gas_params: transaction.gas_params,
            gas_price: transaction.gas_price,
            signature: const_hex::encode_prefixed(signature),
            eip7702_auth,
        };

        Ok(PolygonSignedTransaction {
            unsigned_transaction: transaction,
            op_params,
            op_hash,
        })
    }

    fn process_generate_polygon_address(
        &self,
        data: GenerateAddressData,
    ) -> KeyringResult<EthAddress> {
        let signer = self.generate_polygon_derived_signer(data.derivation_params)?;
        Ok(signer.address())
    }

    // ========================================================================
    // Message Processing
    // ========================================================================

    fn process_message(
        &self,
        msg: KeyringMessage,
    ) {
        match msg {
            KeyringMessage::SignAssetHubTransaction(envelope) => {
                let (req, resp) = envelope.unpack();
                let result = self.process_sign_asset_hub_transaction(req);
                let _unused = resp.send(result);
            },
            KeyringMessage::GenerateAssetHubAddress(envelope) => {
                let (req, resp) = envelope.unpack();
                let result = self.process_generate_asset_hub_address(req);
                let _unused = resp.send(result);
            },
            KeyringMessage::SignPolygonTransaction(envelope) => {
                let (req, resp) = envelope.unpack();
                let result = self.process_sign_polygon_transaction(req);
                let _unused = resp.send(result);
            },
            KeyringMessage::GeneratePolygonAddress(envelope) => {
                let (req, resp) = envelope.unpack();
                let result = self.process_generate_polygon_address(req);
                let _unused = resp.send(result);
            },
            KeyringMessage::SignPolygonPermit(envelope) => {
                let (req, resp) = envelope.unpack();
                let result = self.process_sign_polygon_permit(req);
                let _unused = resp.send(result);
            },
        }
    }
}

struct Envelope<T, R> {
    request: T,
    responder: oneshot::Sender<KeyringResult<R>>,
}

impl<T, R> Envelope<T, R> {
    pub fn new(request: T) -> (Self, ResponseReceiver<R>) {
        let (responder, receiver) = oneshot::channel();

        let envelope = Envelope {
            request,
            responder,
        };

        let response_receiver = ResponseReceiver {
            receiver,
        };

        (envelope, response_receiver)
    }

    pub fn unpack(self) -> (T, oneshot::Sender<KeyringResult<R>>) {
        let Envelope {
            request,
            responder,
        } = self;
        (request, responder)
    }
}

struct ResponseReceiver<R> {
    receiver: oneshot::Receiver<KeyringResult<R>>,
}

impl<R> ResponseReceiver<R> {
    async fn receive(self) -> KeyringResult<R> {
        // TODO: add timeouts?
        self.receiver
            .await
            .inspect_err(|e| {
                tracing::debug!(
                    error.category = crate::utils::logging::category::CHAIN_CLIENT,
                    error.operation = "keyring_response",
                    error.source = ?e,
                    "Failed to receive response from Keyring actor"
                );
            })
            .map_err(|_| KeyringError::MessageTransmissionFailed)?
    }
}

// adding new operations can be simplified using macros
#[expect(clippy::large_enum_variant)]
enum KeyringMessage {
    // Asset Hub messages
    SignAssetHubTransaction(
        Envelope<
            SignTransactionRequestData<AssetHubUnsignedTransaction>,
            AssetHubSignedTransaction,
        >,
    ),
    GenerateAssetHubAddress(Envelope<GenerateAddressData, AssetHubAccountId>),

    // Polygon messages
    SignPolygonPermit(Envelope<SignPermitRequestData, SignedPermit>),
    SignPolygonTransaction(
        Envelope<SignTransactionRequestData<PolygonUnsignedTransaction>, PolygonSignedTransaction>,
    ),
    GeneratePolygonAddress(Envelope<GenerateAddressData, EthAddress>),
}

impl KeyringMessage {
    fn new_sign_asset_hub_transaction(
        data: SignTransactionRequestData<AssetHubUnsignedTransaction>
    ) -> (
        Self,
        ResponseReceiver<AssetHubSignedTransaction>,
    ) {
        let (envelope, response_receiver) = Envelope::new(data);
        (
            Self::SignAssetHubTransaction(envelope),
            response_receiver,
        )
    }

    fn new_generate_asset_hub_address(
        data: GenerateAddressData
    ) -> (
        Self,
        ResponseReceiver<AssetHubAccountId>,
    ) {
        let (envelope, response_receiver) = Envelope::new(data);
        (
            Self::GenerateAssetHubAddress(envelope),
            response_receiver,
        )
    }

    fn new_sign_polygon_permit(
        data: SignPermitRequestData
    ) -> (Self, ResponseReceiver<SignedPermit>) {
        let (envelope, response_receiver) = Envelope::new(data);
        (
            Self::SignPolygonPermit(envelope),
            response_receiver,
        )
    }

    fn new_sign_polygon_transaction(
        data: SignTransactionRequestData<PolygonUnsignedTransaction>
    ) -> (
        Self,
        ResponseReceiver<PolygonSignedTransaction>,
    ) {
        let (envelope, response_receiver) = Envelope::new(data);
        (
            Self::SignPolygonTransaction(envelope),
            response_receiver,
        )
    }

    fn new_generate_polygon_address(
        data: GenerateAddressData
    ) -> (Self, ResponseReceiver<EthAddress>) {
        let (envelope, response_receiver) = Envelope::new(data);
        (
            Self::GeneratePolygonAddress(envelope),
            response_receiver,
        )
    }
}

// Client is wrapped into a separate module to allow mocking and easily
// doubling.
#[cfg_attr(test, expect(dead_code))]
mod client {
    use super::*;

    #[derive(Clone)]
    pub struct KeyringClient {
        tx: mpsc::Sender<KeyringMessage>,
    }

    impl KeyringClient {
        pub(super) fn new(tx: mpsc::Sender<KeyringMessage>) -> Self {
            Self {
                tx,
            }
        }

        async fn send_message_with_response<R: 'static>(
            &self,
            message: KeyringMessage,
            response_receiver: ResponseReceiver<R>,
        ) -> KeyringResult<R> {
            let () = self
                .tx
                .send(message)
                .await
                .inspect_err(|e| {
                    tracing::debug!(
                        error.category = crate::utils::logging::category::CHAIN_CLIENT,
                        error.operation = "keyring_request",
                        error.source = ?e,
                        "Failed to send request to Keyring actor"
                    );
                })
                .map_err(|_| KeyringError::MessageTransmissionFailed)?;

            response_receiver.receive().await
        }

        // ====================================================================
        // Asset Hub Methods
        // ====================================================================

        #[instrument(skip(self, data))]
        pub async fn sign_asset_hub_transaction(
            &self,
            data: SignTransactionRequestData<AssetHubUnsignedTransaction>,
        ) -> KeyringResult<AssetHubSignedTransaction> {
            let params = KeyringMessage::new_sign_asset_hub_transaction(data);
            self.send_message_with_response(params.0, params.1)
                .await
        }

        #[instrument(skip(self, data))]
        pub async fn generate_asset_hub_address(
            &self,
            data: GenerateAddressData,
        ) -> KeyringResult<AssetHubAccountId> {
            let params = KeyringMessage::new_generate_asset_hub_address(data);
            self.send_message_with_response(params.0, params.1)
                .await
        }

        // ====================================================================
        // Polygon Methods
        // ====================================================================

        #[instrument(skip(self, data))]
        pub async fn sign_polygon_permit(
            &self,
            data: SignPermitRequestData,
        ) -> KeyringResult<SignedPermit> {
            let params = KeyringMessage::new_sign_polygon_permit(data);
            self.send_message_with_response(params.0, params.1)
                .await
        }

        #[instrument(skip(self, data))]
        pub async fn sign_polygon_transaction(
            &self,
            data: SignTransactionRequestData<PolygonUnsignedTransaction>,
        ) -> KeyringResult<PolygonSignedTransaction> {
            let params = KeyringMessage::new_sign_polygon_transaction(data);
            self.send_message_with_response(params.0, params.1)
                .await
        }

        #[instrument(skip(self, data))]
        pub async fn generate_polygon_address(
            &self,
            data: GenerateAddressData,
        ) -> KeyringResult<EthAddress> {
            let params = KeyringMessage::new_generate_polygon_address(data);
            self.send_message_with_response(params.0, params.1)
                .await
        }
    }

    #[cfg(test)]
    mockall::mock! {
        pub KeyringClient {
            pub(super) fn new(tx: mpsc::Sender<KeyringMessage>) -> Self;

            pub async fn sign_asset_hub_transaction(
                &self,
                data: SignTransactionRequestData<AssetHubUnsignedTransaction>,
            ) -> KeyringResult<AssetHubSignedTransaction>;

            pub async fn generate_asset_hub_address(
                &self,
                data: GenerateAddressData,
            ) -> KeyringResult<AssetHubAccountId>;

            pub async fn sign_polygon_permit(
                &self,
                data: SignPermitRequestData,
            ) -> KeyringResult<SignedPermit>;

            pub async fn sign_polygon_transaction(
                &self,
                data: SignTransactionRequestData<PolygonUnsignedTransaction>,
            ) -> KeyringResult<PolygonSignedTransaction>;

            pub async fn generate_polygon_address(
                &self,
                data: GenerateAddressData,
            ) -> KeyringResult<EthAddress>;
        }

        impl Clone for KeyringClient {
            fn clone(&self) -> Self;
        }
    }
}
