import { ApiPromise, WsProvider, Keyring } from '@polkadot/api';
import { cryptoWaitReady, decodeAddress, encodeAddress } from '@polkadot/util-crypto';
import { u32 } from '@polkadot/types';
import type { AccountInfo } from '@polkadot/types/interfaces/system';
import type { AssetBalance } from '@polkadot/types/interfaces/assets';
import { BN } from '@polkadot/util';

export async function connectPolkadot(rpcUrl: string): Promise<ApiPromise> {
  const provider = new WsProvider(rpcUrl);
  const api = await ApiPromise.create({ provider });
  await api.isReady;
  return api;
}

export const reverseDecimals = (amount: number, decimals: number): number => {
  return amount / Math.pow(10, decimals);
};

export async function getDotBalance(rpcUrl: string, paymentAccount: string): Promise<number> {
  const provider = new WsProvider(rpcUrl);
  const api = await ApiPromise.create({ provider });

  const accountInfo = await api.query.system.account(paymentAccount);

  // @ts-ignore
  const freeBalance = accountInfo.data.free.toBigInt();

  return Number(freeBalance);
}

export async function getAssetBalance(rpcUrl: string, paymentAccount: string, assetId: number): Promise<number> {
  const provider = new WsProvider(rpcUrl);
  const api = await ApiPromise.create({ provider });
  const decodedAccount = decodeAddress(paymentAccount);

  // Query the balance for the specified asset and account
  const assetIdU32 = new u32(api.registry, assetId);
  const accountInfo = (await api.query.assets.account(assetIdU32, decodedAccount)).toJSON() as { balance: number};

  if (accountInfo) {
    return accountInfo.balance;
  } else {
    return 0;
  }
}

export async function transferFunds(rpcUrl: string, paymentAccount: string, amount: number, assetId?: number) {
  const provider = new WsProvider(rpcUrl);
  const api = await ApiPromise.create({ provider });
  const keyring = new Keyring({ type: 'sr25519' });
  const sender = keyring.addFromUri('//Alice');
  let transfer;
  let signerOptions = {};

  await cryptoWaitReady();

  if (assetId) {
    const adjustedAmount = amount * Math.pow(10, 6);

    transfer = api.tx.assets.transfer(assetId, paymentAccount, adjustedAmount);
    signerOptions = {
      tip: 0,
      assetId: { parents: 0, interior: { X2: [{ palletInstance: 50 }, { generalIndex: assetId }] } }
    };
  } else {
    const adjustedAmount = amount * Math.pow(10, 10);

    transfer = api.tx.balances.transferKeepAlive(paymentAccount, adjustedAmount);
  }

  const unsub = await transfer.signAndSend(sender, signerOptions, async ({ status }) => {
    if (status.isInBlock || status.isFinalized) {
      unsub();
    }
  });

  // Wait for transaction to be included in block
  await new Promise(resolve => setTimeout(resolve, 10000));
}
