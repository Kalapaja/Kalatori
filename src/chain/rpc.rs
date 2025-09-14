//! Blockchain operations that actually require calling the chain

use crate::{
    chain::{
        definitions::{BlockHash, EventFilter},
        utils::{
            asset_balance_query, block_number_query, events_entry_metadata, hashed_key_element,
            system_balance_query, system_properties_to_short_specs,
        },
    },
    definitions::{
        api_v2::{AssetId, BlockNumber, CurrencyProperties, ExtrinsicIndex, Timestamp, TokenKind},
        Balance,
    },
    error::{ChainError, NotHexError},
    utils::unhex,
};
use codec::{DecodeAll, Encode};
use frame_metadata::{
    v15::{
        ExtrinsicMetadata as ExtrinsicMetadataV15, PalletCallMetadata as PalletCallMetadataV15,
        PalletConstantMetadata as PalletConstantMetadataV15,
        PalletErrorMetadata as PalletErrorMetadataV15,
        PalletEventMetadata as PalletEventMetadataV15, PalletMetadata as PalletMetadataV15,
        PalletStorageMetadata as PalletStorageMetadataV15,
        RuntimeApiMetadata as RuntimeApiMetadataV15, RuntimeMetadataV15,
        SignedExtensionMetadata as SignedExtensionMetadataV15, StorageEntryMetadata,
        StorageEntryModifier as StorageEntryModifierV15, StorageEntryType,
        StorageEntryType as StorageEntryTypeV15, StorageHasher as StorageHasherV15,
    },
    v16::{
        ExtrinsicMetadata as ExtrinsicMetadataV16, PalletCallMetadata as PalletCallMetadataV16,
        PalletConstantMetadata as PalletConstantMetadataV16,
        PalletErrorMetadata as PalletErrorMetadataV16,
        PalletEventMetadata as PalletEventMetadataV16, PalletMetadata as PalletMetadataV16,
        PalletStorageMetadata as PalletStorageMetadataV16,
        RuntimeApiMetadata as RuntimeApiMetadataV16, RuntimeMetadataV16,
        StorageEntryMetadata as StorageEntryMetadataV16,
        StorageEntryModifier as StorageEntryModifierV16, StorageEntryType as StorageEntryTypeV16,
        StorageHasher as StorageHasherV16,
        TransactionExtensionMetadata as TransactionExtensionMetadataV16,
    },
    RuntimeMetadata,
};
use hashing::twox_128;
use jsonrpsee::core::client::{ClientT, Subscription, SubscriptionClientT};
use jsonrpsee::rpc_params;
use jsonrpsee::ws_client::WsClient;
use primitive_types::U256;
use scale_info::{form::PortableForm, PortableRegistry, TypeDef, TypeDefPrimitive};
use serde::{de, Deserialize, Deserializer};
use serde_json::{Number, Value};
use std::{collections::HashMap, fmt::Debug};
use substrate_crypto_light::common::AccountId32;
use substrate_parser::{
    cards::{
        Call, Event, ExtendedData, FieldData, PalletSpecificData, ParsedData, Sequence, VariantData,
    },
    decode_all_as_type, decode_as_storage_entry,
    error::UncheckedExtrinsicError,
    special_indicators::SpecialtyUnsignedInteger,
    storage_data::{KeyData, KeyPart},
    unchecked_extrinsic::UncheckedExtrinsic,
    AsMetadata, ResolveType, ShortSpecs,
};

/// To prevent infinite loop while scanning for keys if the node decides to misbehave, limit number
/// of pages
///
/// TODO: add more timeouts
const MAX_KEY_PAGES: usize = 256;

// Pallets

const BALANCES: &str = "Balances";
const TRANSFER: &str = "Transfer";

// Runtime APIs

/// Fetch some runtime version identifier.
///
/// This does not have to be typesafe or anything; this could be used only to check if returned
/// value changes - and reboot the whole connection then, regardless of nature of change.
pub async fn runtime_version_identifier(
    client: &WsClient,
    block: &BlockHash,
) -> Result<Value, ChainError> {
    let value = client
        .request("state_getRuntimeVersion", rpc_params![block.to_string()])
        .await?;
    Ok(value)
}

pub async fn subscribe_blocks(client: &WsClient) -> Result<Subscription<BlockHead>, ChainError> {
    Ok(client
        .subscribe(
            "chain_subscribeFinalizedHeads",
            rpc_params![],
            "chain_unsubscribeFinalizedHeads",
        )
        .await?)
}

pub async fn get_value_from_storage(
    client: &WsClient,
    whole_key: &str,
    block: &BlockHash,
) -> Result<Value, ChainError> {
    let value: Value = client
        .request(
            "state_getStorage",
            rpc_params![whole_key, block.to_string()],
        )
        .await?;
    Ok(value)
}

pub async fn get_keys_from_storage(
    client: &WsClient,
    prefix: &str,
    storage_name: &str,
    block: &BlockHash,
) -> Result<Vec<Value>, ChainError> {
    let mut keys_vec = Vec::new();
    let storage_key_prefix = format!(
        "0x{}{}",
        const_hex::encode(twox_128(prefix.as_bytes())),
        const_hex::encode(twox_128(storage_name.as_bytes()))
    );

    let count = 10;
    // Because RPC API accepts parameters as a sequence and the last 2 parameters are
    // `start_key: Option<StorageKey>` and `hash: Option<Hash>`, API *always* takes `hash` as
    // `storage_key` if the latter is `None` and believes that `hash` is `None` because although
    // `StorageKey` and `Hash` are different types, any `Hash` perfectly deserializes as
    // `StorageKey`. Therefore, `start_key` must always be present to correctly use the
    // `state_getKeysPaged` call with the `hash` parameter.
    let mut start_key: String = "0x".into(); // Start from the beginning

    let params_template = vec![
        serde_json::to_value(storage_key_prefix.clone()).unwrap(),
        serde_json::to_value(count).unwrap(),
    ];

    for _ in 0..MAX_KEY_PAGES {
        let mut params = params_template.clone();
        params.push(serde_json::to_value(start_key.clone()).unwrap());

        params.push(serde_json::to_value(block.to_string()).unwrap());
        if let Ok(keys) = client.request("state_getKeysPaged", params).await {
            if let Value::Array(keys_inside) = &keys {
                if let Some(Value::String(key_string)) = keys_inside.last() {
                    start_key.clone_from(key_string);
                } else {
                    return Ok(keys_vec);
                }
            } else {
                return Ok(keys_vec);
            }

            keys_vec.push(keys);
        } else {
            return Ok(keys_vec);
        }
    }

    Ok(keys_vec)
}

/// fetch genesis hash, must be a hexadecimal string transformable into
/// H256 format
pub async fn genesis_hash(client: &WsClient) -> Result<BlockHash, ChainError> {
    let genesis_hash_request: Value = client
        .request(
            "chain_getBlockHash",
            rpc_params![Value::Number(Number::from(0u8))],
        )
        .await
        .map_err(ChainError::Client)?;
    match genesis_hash_request {
        Value::String(x) => BlockHash::from_str(&x),
        _ => return Err(ChainError::GenesisHashFormat),
    }
}

/// fetch block hash, to request later the metadata and specs for
/// the same block
pub async fn block_hash(
    client: &WsClient,
    number: Option<BlockNumber>,
) -> Result<BlockHash, ChainError> {
    let rpc_params = if let Some(a) = number {
        rpc_params![a]
    } else {
        rpc_params![]
    };
    let block_hash_request: Value = client
        .request("chain_getBlockHash", rpc_params)
        .await
        .map_err(ChainError::Client)?;
    match block_hash_request {
        Value::String(x) => BlockHash::from_str(&x),
        _ => return Err(ChainError::BlockHashFormat),
    }
}

/// fetch metadata at known block
pub async fn metadata(
    client: &WsClient,
    block: &BlockHash,
) -> Result<RuntimeMetadataV15, ChainError> {
    // Prefer V16 metadata first (to capture extrinsic version V5), then fallback to V15.
    let v16_resp: Value = client
        .request(
            "state_call",
            rpc_params![
                "Metadata_metadata_at_version",
                "0x10000000",
                block.to_string()
            ],
        )
        .await
        .map_err(ChainError::Client)?;
    if let Value::String(v16_hex) = v16_resp {
        let raw = unhex(&v16_hex, NotHexError::Metadata)?;
        let maybe = Option::<Vec<u8>>::decode_all(&mut &raw[..])
            .map_err(|_| ChainError::RawMetadataNotDecodeable)?;
        if let Some(bytes) = maybe {
            if !bytes.starts_with(b"meta") {
                return Err(ChainError::NoMetaPrefix);
            }
            match RuntimeMetadata::decode_all(&mut &bytes[4..]) {
                Ok(RuntimeMetadata::V16(runtime_metadata_v16)) => {
                    return Ok(v16_to_v15(runtime_metadata_v16))
                }
                Ok(RuntimeMetadata::V15(runtime_metadata_v15)) => {
                    // Some nodes might respond with older version even when asked for V16.
                    return Ok(runtime_metadata_v15);
                }
                Ok(_) => return Err(ChainError::NoMetadataV15),
                Err(_) => return Err(ChainError::MetadataNotDecodeable),
            }
        }
    }

    // Fallback to V15
    let v15_resp: Value = client
        .request(
            "state_call",
            rpc_params![
                "Metadata_metadata_at_version",
                "0x0f000000",
                block.to_string()
            ],
        )
        .await
        .map_err(ChainError::Client)?;
    match v15_resp {
        Value::String(v15_hex) => {
            let raw = unhex(&v15_hex, NotHexError::Metadata)?;
            let maybe = Option::<Vec<u8>>::decode_all(&mut &raw[..])
                .map_err(|_| ChainError::RawMetadataNotDecodeable)?;
            if let Some(bytes) = maybe {
                if !bytes.starts_with(b"meta") {
                    return Err(ChainError::NoMetaPrefix);
                }
                match RuntimeMetadata::decode_all(&mut &bytes[4..]) {
                    Ok(RuntimeMetadata::V15(runtime_metadata_v15)) => Ok(runtime_metadata_v15),
                    Ok(RuntimeMetadata::V16(runtime_metadata_v16)) => {
                        Ok(v16_to_v15(runtime_metadata_v16))
                    }
                    Ok(_) => Err(ChainError::NoMetadataV15),
                    Err(_) => Err(ChainError::MetadataNotDecodeable),
                }
            } else {
                Err(ChainError::NoMetadataV15)
            }
        }
        _ => Err(ChainError::MetadataFormat),
    }
}

fn map_storage_hasher(h: StorageHasherV16) -> StorageHasherV15 {
    match h {
        StorageHasherV16::Blake2_128 => StorageHasherV15::Blake2_128,
        StorageHasherV16::Blake2_256 => StorageHasherV15::Blake2_256,
        StorageHasherV16::Blake2_128Concat => StorageHasherV15::Blake2_128Concat,
        StorageHasherV16::Twox128 => StorageHasherV15::Twox128,
        StorageHasherV16::Twox256 => StorageHasherV15::Twox256,
        StorageHasherV16::Twox64Concat => StorageHasherV15::Twox64Concat,
        StorageHasherV16::Identity => StorageHasherV15::Identity,
    }
}

fn map_storage_entry_type(
    ty: StorageEntryTypeV16<PortableForm>,
) -> StorageEntryTypeV15<PortableForm> {
    match ty {
        StorageEntryTypeV16::Plain(t) => StorageEntryTypeV15::Plain(t),
        StorageEntryTypeV16::Map {
            hashers,
            key,
            value,
        } => StorageEntryTypeV15::Map {
            hashers: hashers.into_iter().map(map_storage_hasher).collect(),
            key,
            value,
        },
    }
}

fn v16_to_v15(meta: RuntimeMetadataV16) -> RuntimeMetadataV15 {
    let pallets: Vec<PalletMetadataV15<PortableForm>> = meta
        .pallets
        .into_iter()
        .map(|p: PalletMetadataV16<PortableForm>| PalletMetadataV15 {
            name: p.name,
            storage: p.storage.map(|s: PalletStorageMetadataV16<PortableForm>| {
                PalletStorageMetadataV15 {
                    prefix: s.prefix,
                    entries: s
                        .entries
                        .into_iter()
                        .map(
                            |e: StorageEntryMetadataV16<PortableForm>| StorageEntryMetadata {
                                name: e.name,
                                modifier: match e.modifier {
                                    StorageEntryModifierV16::Optional => {
                                        StorageEntryModifierV15::Optional
                                    }
                                    StorageEntryModifierV16::Default => {
                                        StorageEntryModifierV15::Default
                                    }
                                },
                                ty: map_storage_entry_type(e.ty),
                                default: e.default,
                                docs: e.docs,
                            },
                        )
                        .collect(),
                }
            }),
            calls: p
                .calls
                .map(|c: PalletCallMetadataV16<PortableForm>| PalletCallMetadataV15 { ty: c.ty }),
            event: p
                .event
                .map(|e: PalletEventMetadataV16<PortableForm>| PalletEventMetadataV15 { ty: e.ty }),
            constants: p
                .constants
                .into_iter()
                .map(
                    |c: PalletConstantMetadataV16<PortableForm>| PalletConstantMetadataV15 {
                        name: c.name,
                        ty: c.ty,
                        value: c.value,
                        docs: c.docs,
                    },
                )
                .collect(),
            error: p
                .error
                .map(|e: PalletErrorMetadataV16<PortableForm>| PalletErrorMetadataV15 { ty: e.ty }),
            index: p.index,
            docs: p.docs,
        })
        .collect();

    let ext_v16: ExtrinsicMetadataV16<PortableForm> = meta.extrinsic.clone();
    // Prefer the highest version present in the extensions-by-version map,
    // otherwise fall back to the highest advertised version.
    let selected_version = ext_v16
        .transaction_extensions_by_version
        .keys()
        .max()
        .copied()
        .or_else(|| ext_v16.versions.iter().max().copied())
        .unwrap_or(0);
    let call_ty = meta.outer_enums.call_enum_ty;
    let signed_extensions: Vec<SignedExtensionMetadataV15<PortableForm>> = if let Some(indexes) =
        ext_v16
            .transaction_extensions_by_version
            .get(&selected_version)
    {
        indexes
            .iter()
            .filter_map(|idx| ext_v16.transaction_extensions.get((*idx).0 as usize))
            .map(
                |e: &TransactionExtensionMetadataV16<PortableForm>| SignedExtensionMetadataV15 {
                    identifier: e.identifier.clone(),
                    ty: e.ty,
                    additional_signed: e.implicit,
                },
            )
            .collect()
    } else {
        ext_v16
            .transaction_extensions
            .iter()
            .map(
                |e: &TransactionExtensionMetadataV16<PortableForm>| SignedExtensionMetadataV15 {
                    identifier: e.identifier.clone(),
                    ty: e.ty,
                    additional_signed: e.implicit,
                },
            )
            .collect()
    };

    let extrinsic = ExtrinsicMetadataV15 {
        version: selected_version,
        address_ty: ext_v16.address_ty,
        call_ty,
        signature_ty: ext_v16.signature_ty,
        extra_ty: call_ty, // placeholder; unused in our decoding path
        signed_extensions,
    };

    let custom_map = meta
        .custom
        .map
        .into_iter()
        .map(|(key, val)| {
            (
                key,
                frame_metadata::v15::CustomValueMetadata {
                    ty: val.ty,
                    value: val.value,
                },
            )
        })
        .collect();

    let custom = frame_metadata::v15::CustomMetadata { map: custom_map };

    let outer_enums = frame_metadata::v15::OuterEnums {
        call_enum_ty: meta.outer_enums.call_enum_ty,
        event_enum_ty: meta.outer_enums.event_enum_ty,
        error_enum_ty: meta.outer_enums.error_enum_ty,
    };

    RuntimeMetadataV15 {
        types: meta.types,
        pallets,
        extrinsic,
        ty: call_ty,
        apis: meta
            .apis
            .into_iter()
            .map(
                |api: RuntimeApiMetadataV16<PortableForm>| RuntimeApiMetadataV15 {
                    name: api.name,
                    methods: api
                        .methods
                        .into_iter()
                        .map(|m| frame_metadata::v15::RuntimeApiMethodMetadata {
                            name: m.name,
                            inputs: m
                                .inputs
                                .into_iter()
                                .map(|p| frame_metadata::v15::RuntimeApiMethodParamMetadata {
                                    name: p.name,
                                    ty: p.ty,
                                })
                                .collect(),
                            output: m.output,
                            docs: m.docs,
                        })
                        .collect(),
                    docs: api.docs,
                },
            )
            .collect(),
        outer_enums,
        custom,
    }
}

// fetch specs at known block
pub async fn specs(
    client: &WsClient,
    metadata: &RuntimeMetadataV15,
    block: &BlockHash,
) -> Result<ShortSpecs, ChainError> {
    let specs_request: Value = client
        .request("system_properties", rpc_params![block.to_string()])
        .await?;
    match specs_request {
        Value::Object(properties) => system_properties_to_short_specs(&properties, &metadata),
        _ => return Err(ChainError::PropertiesFormat),
    }
}

pub async fn next_block_number(
    blocks: &mut Subscription<BlockHead>,
) -> Result<BlockNumber, ChainError> {
    match blocks.next().await {
        Some(Ok(a)) => Ok(a.number),
        Some(Err(e)) => Err(e.into()),
        None => Err(ChainError::BlockSubscriptionTerminated),
    }
}

pub async fn next_block(
    client: &WsClient,
    blocks: &mut Subscription<BlockHead>,
) -> Result<BlockHash, ChainError> {
    block_hash(&client, Some(next_block_number(blocks).await?)).await
}

#[derive(Deserialize, Debug)]
#[serde(rename_all = "kebab-case")]
pub struct BlockHead {
    //digest: Value,
    //extrinsics_root: String,
    #[serde(deserialize_with = "deserialize_block_number")]
    pub number: BlockNumber,
    //parent_hash: String,
    //state_root: String,
}

fn deserialize_block_number<'d, D: Deserializer<'d>>(d: D) -> Result<BlockNumber, D::Error> {
    let n = U256::deserialize(d)?;

    n.try_into()
        .map_err(|_| de::Error::custom("failed to convert `U256` to a block number"))
}

#[derive(Deserialize)]
pub struct BlockDetails {
    block: Block,
}

#[derive(Deserialize)]
pub struct Block {
    pub block: BlockInner,
}

#[derive(Deserialize)]
pub struct BlockInner {
    pub extrinsics: Vec<String>,
}

/// Get all sufficient assets from a chain
#[expect(clippy::too_many_lines)]
pub async fn assets_set_at_block(
    client: &WsClient,
    block: &BlockHash,
    metadata_v15: &RuntimeMetadataV15,
    rpc_url: &str,
    specs: ShortSpecs,
) -> Result<HashMap<String, CurrencyProperties>, ChainError> {
    let mut assets_set = HashMap::new();
    let chain_name =
        <RuntimeMetadataV15 as AsMetadata<()>>::spec_name_version(metadata_v15)?.spec_name;
    let mut assets_asset_storage_metadata = None;
    let mut assets_metadata_storage_metadata = None;

    for pallet in metadata_v15.pallets.iter() {
        if let Some(storage) = &pallet.storage {
            if storage.prefix == "Assets" {
                for entry in storage.entries.iter() {
                    if entry.name == "Asset" {
                        assets_asset_storage_metadata = Some(entry);
                    }
                    if entry.name == "Metadata" {
                        assets_metadata_storage_metadata = Some(entry);
                    }
                    if assets_asset_storage_metadata.is_some()
                        && assets_metadata_storage_metadata.is_some()
                    {
                        break;
                    }
                }
                break;
            }
        }
    }

    if let (Some(assets_asset_storage_metadata), Some(assets_metadata_storage_metadata)) = (
        assets_asset_storage_metadata,
        assets_metadata_storage_metadata,
    ) {
        let available_keys_assets_asset_vec =
            get_keys_from_storage(client, "Assets", "Asset", block).await?;
        for available_keys_assets_asset in available_keys_assets_asset_vec {
            if let Value::Array(ref keys_array) = available_keys_assets_asset {
                for key in keys_array.iter() {
                    if let Value::String(string_key) = key {
                        let value_fetch = get_value_from_storage(client, string_key, block).await?;
                        if let Value::String(ref string_value) = value_fetch {
                            let key_data = unhex(string_key, NotHexError::StorageKey)?;
                            let value_data = unhex(string_value, NotHexError::StorageValue)?;
                            let storage_entry =
                                decode_as_storage_entry::<&[u8], (), RuntimeMetadataV15>(
                                    &key_data.as_ref(),
                                    &value_data.as_ref(),
                                    &mut (),
                                    assets_asset_storage_metadata,
                                    &metadata_v15.types,
                                )?;
                            let asset_id = {
                                if let KeyData::SingleHash { content } = storage_entry.key {
                                    if let KeyPart::Parsed(extended_data) = content {
                                        if let ParsedData::PrimitiveU32 {
                                            value,
                                            specialty: _,
                                        } = extended_data.data
                                        {
                                            Ok(value)
                                        } else {
                                            Err(ChainError::AssetIdFormat)
                                        }
                                    } else {
                                        Err(ChainError::AssetKeyEmpty)
                                    }
                                } else {
                                    Err(ChainError::AssetKeyNotSingleHash)
                                }
                            }?;
                            let mut verified_sufficient = false;
                            if let ParsedData::Composite(fields) = storage_entry.value.data {
                                for field_data in fields.iter() {
                                    if let Some(field_name) = &field_data.field_name {
                                        if field_name == "is_sufficient" {
                                            if let ParsedData::PrimitiveBool(is_it) =
                                                field_data.data.data
                                            {
                                                verified_sufficient = is_it;
                                            }
                                            break;
                                        }
                                    }
                                }
                            }
                            if verified_sufficient {
                                match &assets_metadata_storage_metadata.ty {
                                    StorageEntryType::Plain(_) => {
                                        return Err(ChainError::AssetMetadataPlain)
                                    }
                                    StorageEntryType::Map {
                                        hashers,
                                        key: key_ty,
                                        value: value_ty,
                                    } => {
                                        if hashers.len() == 1 {
                                            let hasher = &hashers[0];
                                            match metadata_v15
                                                .types
                                                .resolve_ty(key_ty.id, &mut ())?
                                                .type_def
                                            {
                                                TypeDef::Primitive(TypeDefPrimitive::U32) => {
                                                    let key_assets_metadata = format!(
                                                        "0x{}{}{}",
                                                        const_hex::encode(twox_128(
                                                            "Assets".as_bytes()
                                                        )),
                                                        const_hex::encode(twox_128(
                                                            "Metadata".as_bytes()
                                                        )),
                                                        const_hex::encode(hashed_key_element(
                                                            &asset_id.encode(),
                                                            hasher
                                                        ))
                                                    );
                                                    let value_fetch = get_value_from_storage(
                                                        client,
                                                        &key_assets_metadata,
                                                        block,
                                                    )
                                                    .await?;
                                                    if let Value::String(ref string_value) =
                                                        value_fetch
                                                    {
                                                        let value_data = unhex(
                                                            string_value,
                                                            NotHexError::StorageValue,
                                                        )?;
                                                        let value = decode_all_as_type::<
                                                            &[u8],
                                                            (),
                                                            RuntimeMetadataV15,
                                                        >(
                                                            value_ty,
                                                            &value_data.as_ref(),
                                                            &mut (),
                                                            &metadata_v15.types,
                                                        )?;

                                                        let mut name = None;
                                                        let mut symbol = None;
                                                        let mut decimals = None;

                                                        if let ParsedData::Composite(fields) =
                                                            value.data
                                                        {
                                                            for field_data in fields.iter() {
                                                                if let Some(field_name) =
                                                                    &field_data.field_name
                                                                {
                                                                    match field_name.as_str() {
                                                                "name" => match &field_data.data.data {
                                                                    ParsedData::Text{text, specialty: _} => {
                                                                        name = Some(text.to_owned());
                                                                    },
                                                                    ParsedData::Sequence(sequence) => {
                                                                        if let Sequence::U8(bytes) = &sequence.data {
                                                                            if let Ok(name_from_bytes) = String::from_utf8(bytes.to_owned()) {
                                                                                name = Some(name_from_bytes);
                                                                            }
                                                                        }
                                                                    }
                                                                    ParsedData::Composite(fields) => {
                                                                        if fields.len() == 1 {
                                                                            match &fields[0].data.data {
                                                                                ParsedData::Text{text, specialty: _} => {
                                                                                    name = Some(text.to_owned());
                                                                                },
                                                                                ParsedData::Sequence(sequence) => {
                                                                                    if let Sequence::U8(bytes) = &sequence.data {
                                                                                        if let Ok(name_from_bytes) = String::from_utf8(bytes.to_owned()) {
                                                                                            name = Some(name_from_bytes);
                                                                                        }
                                                                                    }
                                                                                },
                                                                                _ => {},
                                                                            }
                                                                        }
                                                                    },
                                                                    _ => {},
                                                                },
                                                                "symbol" => match &field_data.data.data {
                                                                    ParsedData::Text{text, specialty: _} => {
                                                                        symbol = Some(text.to_owned());
                                                                    },
                                                                    ParsedData::Sequence(sequence) => {
                                                                        if let Sequence::U8(bytes) = &sequence.data {
                                                                            if let Ok(symbol_from_bytes) = String::from_utf8(bytes.to_owned()) {
                                                                                symbol = Some(symbol_from_bytes);
                                                                            }
                                                                        }
                                                                    }
                                                                    ParsedData::Composite(fields) => {
                                                                        if fields.len() == 1 {
                                                                            match &fields[0].data.data {
                                                                                ParsedData::Text{text, specialty: _} => {
                                                                                    symbol = Some(text.to_owned());
                                                                                },
                                                                                ParsedData::Sequence(sequence) => {
                                                                                    if let Sequence::U8(bytes) = &sequence.data {
                                                                                        if let Ok(symbol_from_bytes) = String::from_utf8(bytes.to_owned()) {
                                                                                            symbol = Some(symbol_from_bytes);
                                                                                        }
                                                                                    }
                                                                                },
                                                                                _ => {},
                                                                            }
                                                                        }
                                                                    },
                                                                    _ => {},
                                                                },
                                                                "decimals" => {
                                                                    if let ParsedData::PrimitiveU8{value, specialty: _} = field_data.data.data {
                                                                        decimals = Some(value);
                                                                    }
                                                                },
                                                                _ => {},
                                                            }
                                                                }
                                                                if name.is_some()
                                                                    && symbol.is_some()
                                                                    && decimals.is_some()
                                                                {
                                                                    break;
                                                                }
                                                            }
                                                            if let (Some(symbol), Some(decimals)) =
                                                                (symbol, decimals)
                                                            {
                                                                assets_set.insert(
                                                                    symbol,
                                                                    CurrencyProperties {
                                                                        chain_name: chain_name
                                                                            .clone(),
                                                                        kind: TokenKind::Asset,
                                                                        decimals,
                                                                        rpc_url: rpc_url
                                                                            .to_string(),
                                                                        asset_id: Some(asset_id),
                                                                        ss58: specs.base58prefix,
                                                                    },
                                                                );
                                                            }
                                                        } else {
                                                            return Err(
                                                                ChainError::AssetMetadataUnexpected,
                                                            );
                                                        }
                                                    }
                                                }

                                                _ => return Err(ChainError::AssetMetadataType),
                                            }
                                        } else {
                                            return Err(ChainError::AssetMetadataMapSize);
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
    Ok(assets_set)
}

pub async fn asset_balance_at_account(
    client: &WsClient,
    block: &BlockHash,
    metadata_v15: &RuntimeMetadataV15,
    account_id: &AccountId32,
    asset_id: AssetId,
) -> Result<Balance, ChainError> {
    let query = asset_balance_query(metadata_v15, account_id, asset_id)?;

    let value_fetch = get_value_from_storage(client, &query.key, block).await?;
    match value_fetch {
        // Storage key not present => zero balance
        Value::Null => return Ok(Balance(0)),
        Value::String(ref string_value) => {
            let value_data = unhex(string_value, NotHexError::StorageValue)?;
            let value = decode_all_as_type::<&[u8], (), RuntimeMetadataV15>(
                &query.value_ty,
                &value_data.as_ref(),
                &mut (),
                &metadata_v15.types,
            )?;
            if let ParsedData::Composite(fields) = value.data {
                for field in fields.iter() {
                    if let ParsedData::PrimitiveU128 {
                        value,
                        specialty: SpecialtyUnsignedInteger::Balance,
                    } = field.data.data
                    {
                        return Ok(Balance(value));
                    }
                }
                Err(ChainError::AssetBalanceNotFound)
            } else {
                Err(ChainError::AssetBalanceFormat)
            }
        }
        other => Err(ChainError::StorageValueFormat(other)),
    }
}

pub async fn system_balance_at_account(
    client: &WsClient,
    block: &BlockHash,
    metadata_v15: &RuntimeMetadataV15,
    account_id: &AccountId32,
) -> Result<Balance, ChainError> {
    let query = system_balance_query(metadata_v15, account_id)?;

    let value_fetch = get_value_from_storage(client, &query.key, block).await?;
    match value_fetch {
        // Storage key not present => zero balance (account does not exist yet)
        Value::Null => return Ok(Balance(0)),
        Value::String(ref string_value) => {
            let value_data = unhex(string_value, NotHexError::StorageValue)?;
            let value = decode_all_as_type::<&[u8], (), RuntimeMetadataV15>(
                &query.value_ty,
                &value_data.as_ref(),
                &mut (),
                &metadata_v15.types,
            )?;
            // Fallback recursive search for a balance-typed U128
            fn find_balance(data: &ParsedData) -> Option<u128> {
                match data {
                    ParsedData::PrimitiveU128 { value, specialty } => {
                        if *specialty == SpecialtyUnsignedInteger::Balance {
                            Some(*value)
                        } else {
                            None
                        }
                    }
                    ParsedData::Composite(fields) => {
                        for f in fields {
                            if let Some(v) = find_balance(&f.data.data) {
                                return Some(v);
                            }
                        }
                        None
                    }
                    ParsedData::Variant(VariantData { fields, .. }) => {
                        for f in fields {
                            if let Some(v) = find_balance(&f.data.data) {
                                return Some(v);
                            }
                        }
                        None
                    }
                    _ => None,
                }
            }
            if let ParsedData::Composite(fields) = value.data {
                for field in fields.iter() {
                    if field.field_name == Some("data".to_string()) {
                        if let ParsedData::Composite(inner_fields) = &field.data.data {
                            for inner_field in inner_fields.iter() {
                                if inner_field.field_name == Some("free".to_string()) {
                                    if let ParsedData::PrimitiveU128 {
                                        value,
                                        specialty: SpecialtyUnsignedInteger::Balance,
                                    } = inner_field.data.data
                                    {
                                        return Ok(Balance(value));
                                    }
                                }
                            }
                            // If named field lookup failed, fallback to first balance-typed U128
                            if let Some(v) = find_balance(&field.data.data) {
                                return Ok(Balance(v));
                            }
                        }
                    }
                }
                // Fallback: scan entire structure for a balance-typed U128
                if let Some(v) = find_balance(&ParsedData::Composite(fields)) {
                    return Ok(Balance(v));
                }
            }
            Err(ChainError::BalanceNotFound)
        }
        other => Err(ChainError::StorageValueFormat(other)),
    }
}

pub async fn transfer_events(
    client: &WsClient,
    block: &BlockHash,
    metadata_v15: &RuntimeMetadataV15,
) -> Result<(Timestamp, Vec<(Option<(ExtrinsicIndex, Vec<u8>)>, Event)>), ChainError> {
    let events_entry_metadata = events_entry_metadata(metadata_v15)?;
    let events = events_at_block(
        client,
        block,
        Some(EventFilter {
            pallet: BALANCES,
            optional_event_variant: Some(TRANSFER),
        }),
        events_entry_metadata,
        &metadata_v15.types,
    )
    .await?
    .into_iter()
    .chain(
        events_at_block(
            client,
            block,
            Some(EventFilter {
                pallet: "Assets",
                optional_event_variant: Some("Transferred"),
            }),
            events_entry_metadata,
            &metadata_v15.types,
        )
        .await?
        .into_iter(),
    )
    .collect();

    match_extrinsics_with_events_at_block(events, client, block, metadata_v15).await
}

async fn match_extrinsics_with_events_at_block(
    events: Vec<(Option<ExtrinsicIndex>, Event)>,
    client: &WsClient,
    block_hash: &BlockHash,
    metadata_v15: &RuntimeMetadataV15,
) -> Result<(Timestamp, Vec<(Option<(ExtrinsicIndex, Vec<u8>)>, Event)>), ChainError> {
    let block: Block = client
        .request("chain_getBlock", rpc_params!(block_hash.to_string()))
        .await?;
    let extrinsics = block
        .block
        .extrinsics
        .into_iter()
        .map(|encoded| unhex(&encoded, NotHexError::Extrinsic))
        .collect::<Result<Vec<_>, _>>()?;
    let timestamp = extrinsics
        .iter()
        .find_map(|encoded| {
            // Try decoding using provided metadata first.
            let mut try_decode = |meta: &RuntimeMetadataV15| {
                substrate_parser::decode_as_unchecked_extrinsic(&encoded.as_ref(), &mut (), meta)
            };

            let decoded = match try_decode(metadata_v15) {
                Ok(ok) => Ok(ok),
                Err(UncheckedExtrinsicError::VersionMismatch { version_byte, .. }) => {
                    // Fallback: if the chain uses a different extrinsic version (e.g., 5),
                    // retry with a local copy of metadata adjusted to that version.
                    let masked = version_byte & 0b0111_1111;
                    if masked != metadata_v15.extrinsic.version {
                        let mut override_meta = metadata_v15.clone();
                        override_meta.extrinsic.version = masked;
                        try_decode(&override_meta)
                    } else {
                        Err(UncheckedExtrinsicError::VersionMismatch {
                            version_byte,
                            version: masked,
                        })
                    }
                }
                Err(e) => Err(e),
            };

            if let Ok(UncheckedExtrinsic::Unsigned {
                call:
                    Call(PalletSpecificData {
                        pallet_name,
                        variant_name,
                        fields,
                        ..
                    }),
            }) = decoded
            {
                if pallet_name == "Timestamp" && variant_name == "set" {
                    if let Some(FieldData {
                        data:
                            ExtendedData {
                                data: ParsedData::PrimitiveU64 { value, .. },
                                ..
                            },
                        ..
                    }) = fields.into_iter().next()
                    {
                        return Some(Timestamp(value));
                    }
                }
            }

            None
        })
        .ok_or(ChainError::TimestampNotFoundForBlock)?;

    Ok((
        timestamp,
        events
            .into_iter()
            .map(|(extrinsic, event)| {
                let extrinsic_option = extrinsic.and_then(|index| {
                    let index_usize = index.try_into().unwrap();

                    extrinsics
                        .get::<usize>(index_usize)
                        .cloned()
                        .map(|bytes| (index, bytes))
                });

                (extrinsic_option, event)
            })
            .collect(),
    ))
}

async fn events_at_block(
    client: &WsClient,
    block: &BlockHash,
    optional_filter: Option<EventFilter<'_>>,
    events_entry_metadata: &StorageEntryMetadata<PortableForm>,
    types: &PortableRegistry,
) -> Result<Vec<(Option<ExtrinsicIndex>, Event)>, ChainError> {
    let key = format!(
        "0x{}{}",
        const_hex::encode(twox_128("System".as_bytes())),
        const_hex::encode(twox_128("Events".as_bytes()))
    );
    let mut out = Vec::new();
    let data_from_storage = get_value_from_storage(client, &key, block).await?;
    let key_bytes = unhex(&key, NotHexError::StorageValue)?;
    let value_bytes = if let Value::String(data_from_storage) = data_from_storage {
        unhex(&data_from_storage, NotHexError::StorageValue)?
    } else {
        return Err(ChainError::StorageValueFormat(data_from_storage));
    };
    let storage_data = decode_as_storage_entry::<&[u8], (), RuntimeMetadataV15>(
        &key_bytes.as_ref(),
        &value_bytes.as_ref(),
        &mut (),
        events_entry_metadata,
        types,
    )
    .expect("RAM stored metadata access");
    if let ParsedData::SequenceRaw(sequence_raw) = storage_data.value.data {
        for sequence_element in sequence_raw.data {
            let (mut extrinsic_index, mut event_option) = (None, None);

            if let ParsedData::Composite(event_record) = sequence_element {
                for event_record_element in event_record {
                    match event_record_element.field_name.as_deref() {
                        Some("event") => {
                            if let ParsedData::Event(Event(event)) = event_record_element.data.data
                            {
                                if let Some(filter) = &optional_filter {
                                    if let Some(event_variant) = filter.optional_event_variant {
                                        if event.pallet_name == filter.pallet
                                            && event.variant_name == event_variant
                                        {
                                            event_option = Some(Event(event));
                                        }
                                    } else if event.pallet_name == filter.pallet {
                                        event_option = Some(Event(event));
                                    }
                                } else {
                                    event_option = Some(Event(event));
                                }
                            }
                        }
                        Some("phase") => {
                            if let ParsedData::Variant(VariantData {
                                variant_name,
                                fields,
                                ..
                            }) = event_record_element.data.data
                            {
                                if variant_name == "ApplyExtrinsic" {
                                    if let Some(FieldData {
                                        data:
                                            ExtendedData {
                                                data: ParsedData::PrimitiveU32 { value, .. },
                                                ..
                                            },
                                        ..
                                    }) = fields.into_iter().next()
                                    {
                                        extrinsic_index = Some(value);
                                    }
                                }
                            }
                        }
                        _ => {}
                    }
                }
            }

            if let Some(event_some) = event_option {
                out.push((extrinsic_index, event_some));
            }
        }
    }

    Ok(out)
}

pub async fn current_block_number(
    client: &WsClient,
    metadata: &RuntimeMetadataV15,
    block: &BlockHash,
) -> Result<u32, ChainError> {
    let block_number_query = block_number_query(metadata)?;
    let fetched_value = get_value_from_storage(client, &block_number_query.key, block).await?;
    if let Value::String(hex_data) = fetched_value {
        let value_data = unhex(&hex_data, NotHexError::StorageValue)?;
        let value = decode_all_as_type::<&[u8], (), RuntimeMetadataV15>(
            &block_number_query.value_ty,
            &value_data.as_ref(),
            &mut (),
            &metadata.types,
        )?;
        if let ParsedData::PrimitiveU32 {
            value,
            specialty: _,
        } = value.data
        {
            Ok(value)
        } else {
            Err(ChainError::BlockNumberFormat)
        }
    } else {
        Err(ChainError::StorageValueFormat(fetched_value))
    }
}

pub async fn get_nonce(
    client: &WsClient,
    account_id: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let rpc_params = rpc_params![account_id];
    println!("{rpc_params:?}");
    let nonce: Value = client.request("account_nextIndex", rpc_params).await?;
    println!("{nonce:?}");
    Ok(())
}

pub async fn send_stuff(client: &WsClient, data: &str) -> Result<Value, ChainError> {
    let rpc_params = rpc_params![data];
    Ok(client
        .request("author_submitAndWatchExtrinsic", rpc_params)
        .await?)
}
