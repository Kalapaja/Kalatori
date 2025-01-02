import request from 'supertest';
import {getAssetBalance, getDotBalance, reverseDecimals, transferFunds} from '../src/polkadot';

describe('Order Endpoint Blackbox Tests', () => {
  const baseUrl = process.env.DAEMON_HOST;
  if (!baseUrl ) {
    throw new Error('check all environment variables are defined');
  }
  const dotOrderData = {
    amount: 4, // Crucial to test with more than existential amount which is 1 DOT
    currency: 'DOT',
    callback: 'https://example.com/callback'
  };

  const usdcOrderData = {
    amount: 10,
    currency: 'USDC',
    callback: 'https://example.com/callback'
  };

  const invalidOrderAmount = {
    amount: -1,
    currency: 'DOT',
    callback: 'https://example.com/callback'
  };

  const missingOrderAmount = {
    currency: 'DOT',
    callback: 'https://example.com/callback'
  };

  const invalidOrderCurrency = {
    amount: 1,
    currency: 'INVALID',
    callback: 'https://example.com/callback'
  };

  const missingOrderCurrency = {
    amount: 1,
    currency: 'INVALID',
    callback: 'https://example.com/callback'
  };

  const checkOrder = (orderId: any, orderResponseObject:any, orderData: any) => {
    expect(orderResponseObject).toHaveProperty('order', orderId);
    expect(orderResponseObject).toHaveProperty('message', '');
    expect(orderResponseObject).toHaveProperty('recipient');
    expect(orderResponseObject).toHaveProperty('server_info');
    expect(orderResponseObject).toHaveProperty('withdrawal_status', 'waiting');
    expect(orderResponseObject).toHaveProperty('payment_status', 'pending');
    expect(orderResponseObject).toHaveProperty('amount', orderData.amount);

    expect(orderResponseObject).toHaveProperty('callback', orderData.callback);
    expect(orderResponseObject).toHaveProperty('transactions');
    expect(Array.isArray(orderResponseObject.transactions)).toBe(true);
    expect(orderResponseObject).toHaveProperty('payment_account');
    expect(orderResponseObject).toHaveProperty('death');
    expect(orderResponseObject).toHaveProperty('payment_page', '');
    expect(orderResponseObject).toHaveProperty('redirect_url', '');

    expect(orderResponseObject.server_info).toHaveProperty('version');
    expect(orderResponseObject.server_info).toHaveProperty('instance_id');
    expect(orderResponseObject.server_info).toHaveProperty('debug', true);
  }

  const generateRandomOrderId = () => {
    return `order_${Math.random().toString(36).substring(2, 15)}`;
  }

  const createOrder = async (orderId: string, orderData: any, expectedStatus: number = 201) => {
    const response = await request(baseUrl)
      .post(`/v2/order/${orderId}`)
      .send(orderData);
    expect(response.status).toBe(expectedStatus);

    return response.body;
  };

  const getOrderDetails = async (orderId: string) => {
    const response = await request(baseUrl)
      .post(`/v2/order/${orderId}`);
    expect(response.status).toBe(200);
    return response.body;
  };

  const validateTransaction = (transaction: any, expectedCurrency: any) => {
    expect(transaction).toHaveProperty('block_number');
    expect(transaction).toHaveProperty('position_in_block');
    expect(transaction).toHaveProperty('timestamp');
    expect(transaction).toHaveProperty('transaction_bytes');
    expect(transaction).toHaveProperty('sender');
    expect(transaction).toHaveProperty('recipient');
    expect(transaction).toHaveProperty('status', 'finalized');
    expect(transaction).toHaveProperty('currency');
    expect(transaction.currency).toHaveProperty('currency', expectedCurrency.currency);
    expect(transaction.currency).toHaveProperty('chain_name', expectedCurrency.chain_name);
    expect(transaction.currency).toHaveProperty('kind', expectedCurrency.kind);
    expect(transaction.currency).toHaveProperty('decimals', expectedCurrency.decimals);
    expect(transaction.currency).toHaveProperty('rpc_url', expectedCurrency.rpc_url);
  }

  it('should create a new DOT order', async () => {
    const orderId = generateRandomOrderId();
    const createdOrder = await createOrder(orderId, dotOrderData);
    checkOrder(orderId, createdOrder, dotOrderData);

    expect(createdOrder).toHaveProperty('currency');
    expect(createdOrder.currency).toHaveProperty('currency', dotOrderData.currency);
    expect(createdOrder.currency).toHaveProperty('chain_name', 'polkadot');
    expect(createdOrder.currency).toHaveProperty('kind', 'native');
    expect(createdOrder.currency).toHaveProperty('decimals', 10);
    expect(createdOrder.currency).toHaveProperty('rpc_url');
  });

  it('should create a new USDC order', async () => {
    const orderId = generateRandomOrderId();
    const createdOrder = await createOrder(orderId, usdcOrderData);
    checkOrder(orderId, createdOrder, usdcOrderData);

    expect(createdOrder).toHaveProperty('currency');
    expect(createdOrder.currency).toHaveProperty('currency', usdcOrderData.currency);
    expect(createdOrder.currency).toHaveProperty('chain_name', 'statemint');
    expect(createdOrder.currency).toHaveProperty('kind', 'asset');
    expect(createdOrder.currency).toHaveProperty('decimals', 6);
    expect(createdOrder.currency).toHaveProperty('rpc_url');
    expect(createdOrder.currency).toHaveProperty('asset_id', 1337);
  });

  it('should return 400 for invalid amount', async () => {
    const orderId = generateRandomOrderId();
    const response = await request(baseUrl)
      .post(`/v2/order/${orderId}`)
      .send(invalidOrderAmount);

    expect(response.status).toBe(400);
    expect(response.body[0]).toHaveProperty('parameter', 'amount');
    expect(response.body[0]).toHaveProperty('message', expect.stringContaining('less than the currency\'s existential deposit'));
  });

  it('should return 400 for missing amount', async () => {
    const orderId = generateRandomOrderId();
    const response = await request(baseUrl)
      .post(`/v2/order/${orderId}`)
      .send(missingOrderAmount);

    expect(response.status).toBe(400);
    expect(response.body[0]).toHaveProperty('parameter', 'amount');
    expect(response.body[0]).toHaveProperty('message', expect.stringContaining('parameter wasn\'t found'));
  });

  it('should return 400 for invalid currency', async () => {
    const orderId = generateRandomOrderId();
    const response = await request(baseUrl)
      .post(`/v2/order/${orderId}`)
      .send(invalidOrderCurrency);

    expect(response.status).toBe(400);
    expect(response.body[0]).toHaveProperty('parameter', 'currency');
    expect(response.body[0]).toHaveProperty('message', 'provided currency isn\'t supported');
  });

  it('should return 400 for missing currency', async () => {
    const orderId = generateRandomOrderId();
    const response = await request(baseUrl)
      .post(`/v2/order/${orderId}`)
      .send(missingOrderCurrency);

    expect(response.status).toBe(400);
    expect(response.body[0]).toHaveProperty('parameter', 'currency');
    expect(response.body[0]).toHaveProperty('message', 'provided currency isn\'t supported');
  });

  it('should update existing DOT order to be USDC', async () => {
    const orderId = generateRandomOrderId();
    const createdOrder = await createOrder(orderId, dotOrderData);
    checkOrder(orderId, createdOrder, dotOrderData);

    expect(createdOrder).toHaveProperty('currency');
    expect(createdOrder.currency).toHaveProperty('currency', dotOrderData.currency);
    expect(createdOrder.currency).toHaveProperty('chain_name', 'polkadot');
    expect(createdOrder.currency).toHaveProperty('kind', 'native');
    expect(createdOrder.currency).toHaveProperty('decimals', 10);
    expect(createdOrder.currency).toHaveProperty('rpc_url');

    await new Promise(resolve => setTimeout(resolve, 1000));

    const updatedOrder = await createOrder(orderId, usdcOrderData, 200);

    expect(updatedOrder).toHaveProperty('currency');
    expect(updatedOrder.currency).toHaveProperty('currency', usdcOrderData.currency);
    expect(updatedOrder.currency).toHaveProperty('chain_name', 'statemint');
    expect(updatedOrder.currency).toHaveProperty('kind', 'asset');
    expect(updatedOrder.currency).toHaveProperty('decimals', 6);
    expect(updatedOrder.currency).toHaveProperty('rpc_url');
  });

  it('should get DOT order details', async () => {
    const orderId = generateRandomOrderId();
    await createOrder(orderId, dotOrderData);
    const orderDetails = await getOrderDetails(orderId);

    checkOrder(orderId, orderDetails, dotOrderData);

    expect(orderDetails).toHaveProperty('currency');
    expect(orderDetails.currency).toHaveProperty('currency', dotOrderData.currency);
    expect(orderDetails.currency).toHaveProperty('chain_name', 'polkadot');
    expect(orderDetails.currency).toHaveProperty('kind', 'native');
    expect(orderDetails.currency).toHaveProperty('decimals', 10);
    expect(orderDetails.currency).toHaveProperty('rpc_url');
  });

  it('should get USDC order details', async () => {
    const orderId = generateRandomOrderId();
    await createOrder(orderId, usdcOrderData);
    const orderDetails = await getOrderDetails(orderId);
    checkOrder(orderId, orderDetails, usdcOrderData);

    expect(orderDetails).toHaveProperty('currency');
    expect(orderDetails.currency).toHaveProperty('currency', usdcOrderData.currency);
    expect(orderDetails.currency).toHaveProperty('chain_name', 'statemint');
    expect(orderDetails.currency).toHaveProperty('kind', 'asset');
    expect(orderDetails.currency).toHaveProperty('decimals', 6);
    expect(orderDetails.currency).toHaveProperty('rpc_url');
    expect(orderDetails.currency).toHaveProperty('asset_id', 1337);
  });

  it('should return 404 for non-existing order on get order', async () => {
    const nonExistingOrderId = 'nonExistingOrder123';
    const response = await request(baseUrl)
      .post(`/v2/order/${nonExistingOrderId}`);
    expect(response.status).toBe(404);
  });

  it('should create, repay, and automatically withdraw an order in DOT', async () => {
    const orderId = generateRandomOrderId();
    await createOrder(orderId, dotOrderData);
    const orderDetails = await getOrderDetails(orderId);
    const paymentAccount = orderDetails.payment_account;
    expect(paymentAccount).toBeDefined();

    await transferFunds(orderDetails.currency.rpc_url, paymentAccount, dotOrderData.amount);

    // lets wait for the changes to get propagated on chain and app to catch them
    await new Promise(resolve => setTimeout(resolve, 35000));

    const repaidOrderDetails = await getOrderDetails(orderId);

    expect(repaidOrderDetails.transactions.length).toBe(2);

    repaidOrderDetails.transactions.forEach((transaction: any) => {
      validateTransaction(transaction, orderDetails.currency);
    });

    expect(repaidOrderDetails.payment_status).toBe('paid');
    expect(repaidOrderDetails.withdrawal_status).toBe('completed');

    const paymentAccountDotBalance = await getDotBalance(orderDetails.currency.rpc_url, paymentAccount);
    expect(reverseDecimals(paymentAccountDotBalance,10)).toBe(0);
  }, 100000);

  it('should create, repay, and automatically withdraw an order in USDC', async () => {
    const orderId = generateRandomOrderId();
    await createOrder(orderId, usdcOrderData);
    const orderDetails = await getOrderDetails(orderId);
    const paymentAccount = orderDetails.payment_account;
    expect(paymentAccount).toBeDefined();

    await transferFunds(
      orderDetails.currency.rpc_url,
      paymentAccount,
      usdcOrderData.amount,
      orderDetails.currency.asset_id
    );

    // lets wait for the changes to get propagated on chain and app to catch them
    await new Promise(resolve => setTimeout(resolve, 15000));

    const repaidOrderDetails = await getOrderDetails(orderId);

    expect(repaidOrderDetails.transactions.length).toBe(2);

    repaidOrderDetails.transactions.forEach((transaction: any) => {
      validateTransaction(transaction, orderDetails.currency);
    });

    expect(repaidOrderDetails.payment_status).toBe('paid');
    expect(repaidOrderDetails.withdrawal_status).toBe('completed');

    const paymentAccountUsdcBalance = await getAssetBalance(orderDetails.currency.rpc_url, paymentAccount, orderDetails.currency.asset_id);
    expect(reverseDecimals(paymentAccountUsdcBalance, 6)).toBeLessThan(0.1);
  }, 50000);

  it('should not automatically withdraw DOT order until fully repaid', async () => {
    const orderId = generateRandomOrderId();
    await createOrder(orderId, dotOrderData);
    const orderDetails = await getOrderDetails(orderId);
    const paymentAccount = orderDetails.payment_account;
    expect(paymentAccount).toBeDefined();

    const halfAmount = orderDetails.amount/2;

    // Partial repayment
    await transferFunds(
        orderDetails.currency.rpc_url,
        paymentAccount,
        halfAmount,
        orderDetails.currency.asset_id
    );

    // lets wait for the changes to get propagated on chain and app to catch them
    await new Promise(resolve => setTimeout(resolve, 15000));

    const halfAmountBalance = await getDotBalance(orderDetails.currency.rpc_url, paymentAccount);
    expect(reverseDecimals(halfAmountBalance, 10)).toBe(orderDetails.amount/2);

    let repaidOrderDetails = await getOrderDetails(orderId);
    expect(repaidOrderDetails.payment_status).toBe('pending');
    expect(repaidOrderDetails.withdrawal_status).toBe('waiting');

    // Full repayment
    await transferFunds(
        orderDetails.currency.rpc_url,
        paymentAccount,
        halfAmount+5,
        orderDetails.currency.asset_id
    );

    // lets wait for the changes to get propagated on chain and app to catch them
    await new Promise(resolve => setTimeout(resolve, 15000));

    repaidOrderDetails = await getOrderDetails(orderId);
    expect(repaidOrderDetails.payment_status).toBe('paid');
    expect(repaidOrderDetails.withdrawal_status).toBe('completed');

    const paymentAccountDotBalance = await getDotBalance(orderDetails.currency.rpc_url, paymentAccount);
    expect(reverseDecimals(paymentAccountDotBalance, 10)).toBe(0);
  }, 100000);

  it('should not automatically withdraw USDC order until fully repaid', async () => {
    const orderId = generateRandomOrderId();
    await createOrder(orderId, usdcOrderData);
    const orderDetails = await getOrderDetails(orderId);
    const paymentAccount = orderDetails.payment_account;
    expect(paymentAccount).toBeDefined();

    const halfAmount = orderDetails.amount/2;

    // Partial repayment
    await transferFunds(
      orderDetails.currency.rpc_url,
      paymentAccount,
      halfAmount,
      orderDetails.currency.asset_id
    );

    // lets wait for the changes to get propagated on chain and app to catch them
    await new Promise(resolve => setTimeout(resolve, 15000));

    const halfAmountBalance = await getAssetBalance(orderDetails.currency.rpc_url, paymentAccount, orderDetails.currency.asset_id);
    expect(reverseDecimals(halfAmountBalance, 6)).toBe(halfAmount);

    let repaidOrderDetails = await getOrderDetails(orderId);
    expect(repaidOrderDetails.payment_status).toBe('pending');
    expect(repaidOrderDetails.withdrawal_status).toBe('waiting');

    // Full repayment
    await transferFunds(
      orderDetails.currency.rpc_url,
      paymentAccount,
      halfAmount,
      orderDetails.currency.asset_id
    );

    // lets wait for the changes to get propagated on chain and app to catch them
    await new Promise(resolve => setTimeout(resolve, 15000));

    repaidOrderDetails = await getOrderDetails(orderId);
    expect(repaidOrderDetails.payment_status).toBe('paid');
    expect(repaidOrderDetails.withdrawal_status).toBe('completed');

    const paymentAccountUsdcBalance = await getAssetBalance(orderDetails.currency.rpc_url, paymentAccount, orderDetails.currency.asset_id);
    expect(reverseDecimals(paymentAccountUsdcBalance, 6)).toBeLessThan(0.1);;
  }, 100000);

  it('should not update order if received payment in wrong currency', async () => {
    const orderId = generateRandomOrderId();
    await createOrder(orderId, usdcOrderData);
    const orderDetails = await getOrderDetails(orderId);
    const paymentAccount = orderDetails.payment_account;
    expect(paymentAccount).toBeDefined();

    const assetId = 1984; // Different asset ID to simulate wrong currency
    await transferFunds(
      orderDetails.currency.rpc_url,
      paymentAccount,
      usdcOrderData.amount,
      assetId
    );

    // lets wait for the changes to get propagated on chain and app to catch them
    await new Promise(resolve => setTimeout(resolve, 15000));

    const repaidOrderDetails = await getOrderDetails(orderId);
    expect(repaidOrderDetails.payment_status).toBe('pending');
    expect(repaidOrderDetails.withdrawal_status).toBe('waiting');
  }, 50000);

  it('should be able to force withdraw partially repayed DOT order', async () => {
    const orderId = generateRandomOrderId();
    await createOrder(orderId, dotOrderData);
    const orderDetails = await getOrderDetails(orderId);
    const paymentAccount = orderDetails.payment_account;
    expect(paymentAccount).toBeDefined();

    await transferFunds(orderDetails.currency.rpc_url, paymentAccount, dotOrderData.amount/2);

    // lets wait for the changes to get propagated on chain and app to catch them
    await new Promise(resolve => setTimeout(resolve, 15000));

    const halfAmountBalance = await getDotBalance(orderDetails.currency.rpc_url, paymentAccount);
    expect(reverseDecimals(halfAmountBalance, 10)).toBe(orderDetails.amount/2);

    const partiallyRepaidOrderDetails = await getOrderDetails(orderId);
    expect(partiallyRepaidOrderDetails.payment_status).toBe('pending');
    expect(partiallyRepaidOrderDetails.withdrawal_status).toBe('waiting');

    const response = await request(baseUrl)
        .post(`/v2/order/${orderId}/forceWithdrawal`);
    expect(response.status).toBe(201);

    let forcedOrderDetails = await getOrderDetails(orderId);
    expect(forcedOrderDetails.payment_status).toBe('pending');
    expect(forcedOrderDetails.withdrawal_status).toBe('forced');

    await new Promise(resolve => setTimeout(resolve, 5000));

    const paymentAccountDotBalance = await getDotBalance(orderDetails.currency.rpc_url, paymentAccount);
    expect(reverseDecimals(paymentAccountDotBalance, 10)).toBe(0);
  }, 100000);

  it('should be able to force withdraw partially repayed USDC order', async () => {
    const orderId = generateRandomOrderId();
    await createOrder(orderId, usdcOrderData);
    const orderDetails = await getOrderDetails(orderId);
    const paymentAccount = orderDetails.payment_account;
    expect(paymentAccount).toBeDefined();

    const halfAmount = orderDetails.amount/2;

    // Partial repayment
    await transferFunds(
        orderDetails.currency.rpc_url,
        paymentAccount,
        halfAmount,
        orderDetails.currency.asset_id
    );

    // lets wait for the changes to get propagated on chain and app to catch them
    await new Promise(resolve => setTimeout(resolve, 15000));

    const halfAmountBalance = await getAssetBalance(orderDetails.currency.rpc_url, paymentAccount, orderDetails.currency.asset_id);
    expect(reverseDecimals(halfAmountBalance, 6)).toBe(halfAmount);

    const partiallyRepaidOrderDetails = await getOrderDetails(orderId);
    expect(partiallyRepaidOrderDetails.payment_status).toBe('pending');
    expect(partiallyRepaidOrderDetails.withdrawal_status).toBe('waiting');

    const response = await request(baseUrl)
        .post(`/v2/order/${orderId}/forceWithdrawal`);
    expect(response.status).toBe(201);

    // lets wait for the changes to get propagated on chain and app to catch them
    await new Promise(resolve => setTimeout(resolve, 15000));

    let forcedOrderDetails = await getOrderDetails(orderId);
    expect(forcedOrderDetails.payment_status).toBe('pending');
    expect(forcedOrderDetails.withdrawal_status).toBe('forced');

    const paymentAccountUsdcBalance = await getAssetBalance(orderDetails.currency.rpc_url, paymentAccount, orderDetails.currency.asset_id);

    expect(reverseDecimals(paymentAccountUsdcBalance, 6)).toBeLessThan(0.1);
  }, 100000);

  it('should return 404 for non-existing order on force withdrawal', async () => {
    const nonExistingOrderId = 'nonExistingOrder123';
    const response = await request(baseUrl)
      .post(`/v2/order/${nonExistingOrderId}/forceWithdrawal`);
    expect(response.status).toBe(404);
  });
});
