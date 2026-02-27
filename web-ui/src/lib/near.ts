import { setupWalletSelector } from '@near-wallet-selector/core';
import { setupModal } from '@near-wallet-selector/modal-ui';
import { setupMyNearWallet } from '@near-wallet-selector/my-near-wallet';
import { setupMeteorWallet } from '@near-wallet-selector/meteor-wallet';
import { setupIntearWallet } from '@near-wallet-selector/intear-wallet';
import { setupHotWallet } from '@near-wallet-selector/hot-wallet';
import { setupNearMobileWallet } from '@near-wallet-selector/near-mobile-wallet';
import type { WalletSelector, AccountState } from '@near-wallet-selector/core';
import { actionCreators } from '@near-js/transactions';

// ==================== Type Definitions for NEAR Result Parsing ====================

/** NEAR transaction status with either Failure or SuccessValue */
interface ExecutionStatus {
  SuccessValue?: string;
  Failure?: unknown;
}

/** NEAR execution outcome */
interface ExecutionOutcome {
  status: ExecutionStatus;
}

/** NEAR receipt outcome (from transaction result) */
interface ReceiptOutcome {
  outcome?: ExecutionOutcome;
}

/** NEAR transaction outcome */
interface TransactionOutcome {
  outcome?: ExecutionOutcome;
}

/** NEAR signAndSendTransaction result structure */
interface TransactionResult {
  receipts_outcome?: ReceiptOutcome[];
  transaction?: TransactionOutcome;
  status?: ExecutionStatus;
}

// Configuration
const NETWORK_ID = process.env.NEXT_PUBLIC_NETWORK_ID || 'mainnet';

const OUTLAYER_CONTRACT = process.env.NEXT_PUBLIC_OUTLAYER_CONTRACT ||
  (NETWORK_ID === 'testnet' ? 'outlayer.testnet' : 'outlayer.near');
const PROJECT_ID = process.env.NEXT_PUBLIC_PROJECT_ID || 'near-forms';
const SECRETS_PROFILE = process.env.NEXT_PUBLIC_SECRETS_PROFILE || 'default';
const SECRETS_ACCOUNT_ID = process.env.NEXT_PUBLIC_SECRETS_ACCOUNT_ID || '';
const USE_SECRETS = process.env.NEXT_PUBLIC_USE_SECRETS !== 'false'; // default: true

// Parse deposit from env var (in NEAR), convert to yoctoNEAR (1 NEAR = 10^24 yoctoNEAR)
const OUTLAYER_DEPOSIT_NEAR = parseFloat(process.env.NEXT_PUBLIC_OUTLAYER_DEPOSIT_NEAR || '0.025');
if (isNaN(OUTLAYER_DEPOSIT_NEAR) || OUTLAYER_DEPOSIT_NEAR <= 0) {
  throw new Error(`Invalid NEXT_PUBLIC_OUTLAYER_DEPOSIT_NEAR: "${process.env.NEXT_PUBLIC_OUTLAYER_DEPOSIT_NEAR}" (must be a positive number)`);
}
const OUTLAYER_DEPOSIT_YOCTO = BigInt(Math.floor(OUTLAYER_DEPOSIT_NEAR * 1e24));

let selector: WalletSelector | null = null;
let modal: ReturnType<typeof setupModal> | null = null;
let selectorPromise: Promise<WalletSelector> | null = null;
// Prevent overlapping transactions — Meteor wallet's popup reuses the window
// and shows "DApp request has already been submitted" if a second request arrives
// before the first completes.
let pendingTransaction: Promise<any> | null = null;

export async function initWalletSelector(): Promise<WalletSelector> {
  if (!selectorPromise) {
    selectorPromise = setupWalletSelector({
      network: NETWORK_ID as 'mainnet' | 'testnet',
      modules: [
        setupMyNearWallet(),
        setupMeteorWallet(),
        setupIntearWallet(),
        setupHotWallet(),
        setupNearMobileWallet(),
      ],
    }).then(s => {
      // Note: We intentionally omit contractId to prevent wallets from creating
      // a function call access key during sign-in (saves gas). The app works
      // without it since we use signAndSendTransaction which prompts for approval.
      selector = s;
      modal = setupModal(s, {});
      return s;
    });
  }
  return selectorPromise;
}

export function showModal() {
  if (modal) {
    modal.show();
  }
}

export async function getAccounts(): Promise<AccountState[]> {
  if (!selector) {
    await initWalletSelector();
  }
  return selector!.store.getState().accounts;
}

export async function signOut(): Promise<void> {
  if (!selector) return;
  // Check if there's a connected wallet before trying to sign out
  const accounts = selector.store.getState().accounts;
  if (accounts.length === 0) {
    return;
  }
  const wallet = await selector.wallet();
  await wallet.signOut();
}

// Call OutLayer via NEAR transaction
// The signer is authenticated via the blockchain transaction (env::signer_account_id() in WASI)
export async function callOutLayer(action: string, params: Record<string, any>): Promise<any> {
  if (pendingTransaction) {
    throw new Error('A transaction is already in progress. Please wait for it to complete.');
  }

  const accounts = await getAccounts();
  if (accounts.length === 0) {
    throw new Error('Not connected');
  }

  if (!selector) {
    throw new Error('Wallet not initialized');
  }

  const wallet = await selector.wallet();

  // Build input data for WASI module
  // Note: action must come last to prevent params from overriding it
  const inputData = JSON.stringify({
    ...params,
    action,
  });

  // Build request_execution call
  const requestArgs: Record<string, any> = {
    source: {
      Project: {
        project_id: PROJECT_ID,
        version_key: null,  // Use active version
      },
    },
    input_data: inputData,
    resource_limits: {
      max_instructions: 2000000000,
      max_memory_mb: 512,
      max_execution_seconds: 120,
    },
    response_format: 'Json',
  };

  // Add secrets_ref if configured and enabled
  if (USE_SECRETS && SECRETS_ACCOUNT_ID) {
    requestArgs.secrets_ref = {
      profile: SECRETS_PROFILE,
      account_id: SECRETS_ACCOUNT_ID,
    };
  }

  // Create the function call action
  const action_call = actionCreators.functionCall(
    'request_execution',
    requestArgs,
    BigInt('300000000000000'), // 300 TGas
    OUTLAYER_DEPOSIT_YOCTO
  );

  // Send transaction (with lock to prevent overlapping Meteor wallet requests)
  let result;
  try {
    pendingTransaction = wallet.signAndSendTransaction({
      receiverId: OUTLAYER_CONTRACT,
      actions: [action_call],
    });
    result = await pendingTransaction;
  } finally {
    pendingTransaction = null;
  }

  // Check for wallet cancellation (wallet returns undefined)
  if (result === undefined || result === null) {
    throw new Error('Transaction cancelled');
  }

  // Extract the result from transaction
  // The result is in the SuccessValue of the final execution outcome
  const txResult = result as TransactionResult;
  let successValue: string | null = null;

  if (txResult && typeof txResult === 'object') {
    // Try receipts_outcome array (common in NEAR)
    // First pass: check for any failures
    if (txResult.receipts_outcome && Array.isArray(txResult.receipts_outcome)) {
      for (const receipt of txResult.receipts_outcome) {
        if (receipt?.outcome?.status?.Failure) {
          const failure = receipt.outcome.status.Failure;
          throw new Error(`OutLayer execution failed: ${JSON.stringify(failure)}`);
        }
      }
      // Second pass: extract success value (take last non-empty SuccessValue)
      for (const receipt of txResult.receipts_outcome) {
        if (receipt?.outcome?.status?.SuccessValue !== undefined) {
          const val = receipt.outcome.status.SuccessValue;
          if (val !== '') {
            successValue = val;
          }
        }
      }
    }

    // Try transaction.outcome
    if (successValue === null && txResult.transaction?.outcome?.status?.SuccessValue !== undefined) {
      successValue = txResult.transaction.outcome.status.SuccessValue;
    }

    // Try direct status
    if (successValue === null && txResult.status?.SuccessValue !== undefined) {
      successValue = txResult.status.SuccessValue;
    }
  }

  if (successValue === null) {
    throw new Error('No result from OutLayer execution');
  }

  // Treat empty SuccessValue as void/empty result (valid in NEAR protocol)
  if (successValue === '') {
    return {};
  }

  // Decode base64 result
  try {
    const decoded = atob(successValue);
    const resultBytes = new Uint8Array(decoded.length);
    for (let i = 0; i < decoded.length; i++) {
      resultBytes[i] = decoded.charCodeAt(i);
    }

    return JSON.parse(new TextDecoder().decode(resultBytes));
  } catch (e) {
    throw new Error(`Failed to decode OutLayer result: ${e}`);
  }
}
