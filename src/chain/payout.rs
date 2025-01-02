//! Separate engine for payout process.
//!
//! This is so unimportant for smooth SALES process, that it should be given the lowest possible
//! priority, optimized for lazy and very delayed process, and in some cases might be disabeled
//! altogether (TODO)

use crate::{
    chain::{
        definitions::Invoice,
        rpc::{block_hash, current_block_number, send_stuff},
        tracker::ChainWatcher,
        utils::{
            construct_batch_transaction, construct_single_asset_transfer_call,
            construct_single_balance_transfer_call, AssetTransferConstructor,
            BalanceTransferConstructor,
        },
    },
    database::{TransactionInfoDb, TransactionInfoDbInner, TxKind},
    definitions::{
        api_v2::{Amount, TokenKind, TxStatus},
        Balance,
    },
    error::ChainError,
    signer::Signer,
    state::State,
};
use frame_metadata::v15::RuntimeMetadataV15;
use jsonrpsee::ws_client::WsClientBuilder;
use substrate_constructor::fill_prepare::TypeContentToFill;
use substrate_crypto_light::common::AsBase58;

/// Single function that should completely handle payout attmept. Just do not call anything else.
///
/// TODO: make this an additional runner independent from chain monitors
#[expect(clippy::too_many_lines)]
pub async fn payout(
    rpc: String,
    order: Invoice,
    state: State,
    chain: ChainWatcher,
    signer: Signer,
) -> Result<(), ChainError> {
    // TODO: make this retry and rotate RPCs maybe
    //
    // after some retries record a failure
    if let Ok(client) = WsClientBuilder::default().build(&rpc).await {
        let block = block_hash(&client, None).await?; // TODO should retry instead
        let block_number = current_block_number(&client, &chain.metadata, &block).await?;
        let balance = order.balance(&client, &chain, &block).await?; // TODO same
        let loss_tolerance = 20000; // TODO: replace with multiple of existential
                                    // TODO: add upper limit for transactions that would require manual intervention
                                    // just because it was found to be needed with non-crypto trade, who knows why?
        let currency = chain
            .assets
            .get(&order.currency.currency)
            .ok_or_else(|| ChainError::InvalidCurrency(order.currency.currency.clone()))?;
        let order_amount = Balance::parse(order.amount, order.currency.decimals);

        // Payout operation logic
        let (transactions, final_amount) = if balance.0.abs_diff(order_amount.0) <= loss_tolerance
        // modulus(balance-order.amount) <= loss_tolerance
        {
            tracing::info!("Regular withdrawal");
            (
                match currency.kind {
                    TokenKind::Native => {
                        let balance_transfer_constructor = BalanceTransferConstructor {
                            amount: order_amount.0,
                            to_account: &order.recipient,
                            is_clearing: true,
                        };
                        vec![construct_single_balance_transfer_call(
                            &chain.metadata,
                            &balance_transfer_constructor,
                        )?]
                    }
                    TokenKind::Asset => {
                        let asset_transfer_constructor = AssetTransferConstructor {
                            asset_id: currency.asset_id.ok_or(ChainError::AssetId)?,
                            amount: order_amount.0 - loss_tolerance,
                            to_account: &order.recipient,
                        };
                        vec![construct_single_asset_transfer_call(
                            &chain.metadata,
                            &asset_transfer_constructor,
                        )?]
                    }
                },
                order_amount.0,
            )
        } else {
            tracing::info!("Overpayment or forced");
            // We will transfer all the available balance
            // TODO smarter handling and returns probably

            (
                match currency.kind {
                    TokenKind::Native => {
                        let balance_transfer_constructor = BalanceTransferConstructor {
                            amount: balance.0,
                            to_account: &order.recipient,
                            is_clearing: true,
                        };
                        vec![construct_single_balance_transfer_call(
                            &chain.metadata,
                            &balance_transfer_constructor,
                        )?]
                    }
                    TokenKind::Asset => {
                        let asset_transfer_constructor = AssetTransferConstructor {
                            asset_id: currency.asset_id.ok_or(ChainError::AssetId)?,
                            amount: balance.0 - loss_tolerance,
                            to_account: &order.recipient,
                        };
                        vec![construct_single_asset_transfer_call(
                            &chain.metadata,
                            &asset_transfer_constructor,
                        )?]
                    }
                },
                balance.0,
            )
        };

        let mut batch_transaction = construct_batch_transaction(
            &chain.metadata,
            chain.genesis_hash,
            order.address,
            &transactions,
            block,
            block_number,
            0,
            currency.asset_id,
        )?;

        let sign_this = batch_transaction
            .sign_this()
            .ok_or(ChainError::TransactionNotSignable(format!(
                "{batch_transaction:?}"
            )))?;

        let signature = signer.sign(order.id.clone(), sign_this).await?;

        if let TypeContentToFill::Variant(ref mut multisig) = batch_transaction.signature.content {
            if let TypeContentToFill::ArrayU8(ref mut sr25519) =
                multisig.selected.fields_to_fill[0].type_to_fill.content
            {
                sr25519.content = signature.0.to_vec();
            }
        }

        let extrinsic = batch_transaction
            .send_this_signed::<(), RuntimeMetadataV15>(&chain.metadata)?
            .ok_or(ChainError::NothingToSend)?;
        let encoded_extrinsic = const_hex::encode_prefixed(extrinsic);

        state
            .record_transaction(
                TransactionInfoDb {
                    transaction_bytes: encoded_extrinsic.clone(),
                    inner: TransactionInfoDbInner {
                        finalized_tx_timestamp: None,
                        finalized_tx: None,
                        sender: signer.public(order.id.clone(), 42).await?,
                        recipient: order.recipient.to_base58_string(42),
                        amount: Amount::Exact(
                            Balance(final_amount).format(order.currency.decimals),
                        ),
                        currency: order.currency,
                        status: TxStatus::Pending,
                        kind: TxKind::Withdrawal,
                    },
                },
                order.id.clone(),
            )
            .await
            .map_err(|_| ChainError::TransactionNotSaved)?;

        send_stuff(&client, &encoded_extrinsic).await?;

        state.order_withdrawn(order.id).await;
        // TODO obvious
    }
    Ok(())
}
