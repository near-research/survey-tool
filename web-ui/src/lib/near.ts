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
export interface TransactionResult {
  receipts_outcome?: ReceiptOutcome[];
  transaction?: TransactionOutcome;
  status?: ExecutionStatus;
}

/** OutLayer WASI module response (action-dependent) */
export interface OutLayerResult {
  success?: boolean;
  error?: string;
  encrypted_payload?: string;
  [key: string]: unknown;
}

/** OutLayer request_execution contract call arguments */
interface RequestExecutionArgs {
  source: {
    Project: {
      project_id: string;
      version_key: null;
    };
  };
  input_data: string;
  resource_limits: {
    max_instructions: number;
    max_memory_mb: number;
    max_execution_seconds: number;
  };
  response_format: string;
  secrets_ref?: {
    profile: string;
    account_id: string;
  };
}

const NETWORK_ID = process.env.NEXT_PUBLIC_NETWORK_ID || 'mainnet';

const OUTLAYER_CONTRACT = process.env.NEXT_PUBLIC_OUTLAYER_CONTRACT ||
  (NETWORK_ID === 'testnet' ? 'outlayer.testnet' : 'outlayer.near');
function getProjectId(): string {
  const id = process.env.NEXT_PUBLIC_PROJECT_ID;
  if (!id) {
    throw new Error('NEXT_PUBLIC_PROJECT_ID is not set. Set this to your OutLayer project ID (e.g., "account.testnet/near-forms").');
  }
  return id;
}
const SECRETS_PROFILE = process.env.NEXT_PUBLIC_SECRETS_PROFILE || 'default';
const SECRETS_ACCOUNT_ID = process.env.NEXT_PUBLIC_SECRETS_ACCOUNT_ID || '';
const USE_SECRETS = process.env.NEXT_PUBLIC_USE_SECRETS !== 'false'; // default: true

import { nearToYocto } from './form-helpers';

// Wallet error messages used to detect user cancellation / window close.
// Shared across pages to prevent typo drift.
export const WALLET_ERR_CANCELLED = 'Transaction cancelled';
export const WALLET_ERR_WINDOW_CLOSED = ['User closed the window', 'user closed the window'] as const;

const OUTLAYER_DEPOSIT_STR = process.env.NEXT_PUBLIC_OUTLAYER_DEPOSIT_NEAR || '0.025';
const OUTLAYER_DEPOSIT_YOCTO = nearToYocto(OUTLAYER_DEPOSIT_STR);

let selector: WalletSelector | null = null;
let modal: ReturnType<typeof setupModal> | null = null;
let selectorPromise: Promise<WalletSelector> | null = null;
// Prevent overlapping transactions — Meteor wallet's popup reuses the window
// and shows "DApp request has already been submitted" if a second request arrives
// before the first completes.
let pendingTransaction: Promise<unknown> | null = null;

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
    }).catch(err => {
      selectorPromise = null;
      throw err;
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

/**
 * Extract the SuccessValue from a NEAR transaction result.
 * Checks receipts_outcome (last non-empty), then transaction.outcome, then top-level status.
 * Throws on any receipt failure or if no success value is found.
 */
export function extractSuccessValue(txResult: TransactionResult): string {
  let successValue: string | null = null;

  if (txResult && typeof txResult === 'object') {
    // Try receipts_outcome array (common in NEAR)
    // First pass: check for any failures
    if (txResult.receipts_outcome && Array.isArray(txResult.receipts_outcome)) {
      for (const receipt of txResult.receipts_outcome) {
        if (receipt?.outcome?.status?.Failure) {
          const failure = receipt.outcome.status.Failure;
          console.error('OutLayer execution failed:', failure);
          throw new Error('OutLayer execution failed');
        }
      }
      // Second pass: extract success value (take last non-empty SuccessValue)
      // OutLayer's request_execution may produce multiple receipts via cross-contract calls.
      // We take the last non-empty SuccessValue, which in practice contains the WASI output.
      // ASSUMPTION: The WASI output receipt is always the last non-empty one in the chain.
      // If OutLayer's cross-contract call ordering changes, this logic may need updating.
      let selectedIndex = -1;
      for (let i = 0; i < txResult.receipts_outcome.length; i++) {
        const receipt = txResult.receipts_outcome[i];
        if (receipt?.outcome?.status?.SuccessValue !== undefined) {
          const val = receipt.outcome.status.SuccessValue;
          if (val !== '') {
            successValue = val;
            selectedIndex = i;
          }
        }
      }
      if (selectedIndex >= 0) {
        console.debug(
          `OutLayer result: used receipt ${selectedIndex} of ${txResult.receipts_outcome.length}`
        );
      }
    }

    if (successValue === null && txResult.transaction?.outcome?.status?.SuccessValue !== undefined) {
      successValue = txResult.transaction.outcome.status.SuccessValue;
    }

    if (successValue === null && txResult.status?.SuccessValue !== undefined) {
      successValue = txResult.status.SuccessValue;
    }
  }

  if (successValue === null) {
    throw new Error('No result from OutLayer execution');
  }

  return successValue;
}

// Call OutLayer via NEAR transaction
// The signer is authenticated via the blockchain transaction (env::signer_account_id() in WASI)
export async function callOutLayer(action: string, params: Record<string, unknown>): Promise<OutLayerResult> {
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

  if ('action' in params) {
    throw new Error('params must not contain "action" key — it is set automatically');
  }
  const inputData = JSON.stringify({
    ...params,
    action,
  });

  const requestArgs: RequestExecutionArgs = {
    source: {
      Project: {
        project_id: getProjectId(),
        version_key: null,
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
  } else if (USE_SECRETS) {
    throw new Error('Secrets misconfigured: NEXT_PUBLIC_USE_SECRETS is true but NEXT_PUBLIC_SECRETS_ACCOUNT_ID is empty. Set the account ID or disable secrets with NEXT_PUBLIC_USE_SECRETS=false.');
  }

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
  const txResult = result as TransactionResult;
  const successValue = extractSuccessValue(txResult);

  // Empty SuccessValue may indicate WASI module panic/OOM — return explicit error
  if (successValue === '') {
    return { error: 'Empty response from OutLayer' };
  }

  try {
    const decoded = atob(successValue);
    const resultBytes = new Uint8Array(decoded.length);
    for (let i = 0; i < decoded.length; i++) {
      resultBytes[i] = decoded.charCodeAt(i);
    }

    const parsed = JSON.parse(new TextDecoder().decode(resultBytes));

    // Validate result has expected WASI output shape (success/error/encrypted_payload).
    // If shape doesn't match, the receipt ordering assumption may have broken — warn but
    // still return the result to avoid blocking on OutLayer changes.
    if (parsed && typeof parsed === 'object' && !Array.isArray(parsed)) {
      const hasExpectedField = 'success' in parsed || 'error' in parsed || 'encrypted_payload' in parsed;
      if (!hasExpectedField) {
        console.warn('OutLayer result has unexpected shape (no success/error/encrypted_payload field):', Object.keys(parsed));
      }
    }

    return parsed;
  } catch (e) {
    throw new Error(`Failed to decode OutLayer result: ${e}`);
  }
}
