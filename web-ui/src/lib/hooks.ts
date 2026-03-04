import { useEffect, useState, useRef, useCallback } from 'react';
import { sanitizeUserError } from './form-helpers';

interface FetchState<T> {
  data: T | null;
  loading: boolean;
  error: string | null;
}

interface UseFetchOptions {
  /** Timeout in milliseconds (default: 10000) */
  timeout?: number;
  /** Whether to skip the fetch (e.g., when dependencies aren't ready) */
  skip?: boolean;
}

/**
 * Fetch data with timeout, abort on unmount, and loading/error state.
 */
export function useFetchWithTimeout<T>(
  url: string | null,
  options: UseFetchOptions = {}
): FetchState<T> {
  const { timeout = 10_000, skip = false } = options;
  const [state, setState] = useState<FetchState<T>>({
    data: null,
    loading: !skip && !!url,
    error: null,
  });

  useEffect(() => {
    if (skip || !url) {
      setState(prev => ({ ...prev, loading: false }));
      return;
    }

    const controller = new AbortController();
    const timeoutId = setTimeout(() => controller.abort(), timeout);

    setState({ data: null, loading: true, error: null });

    const doFetch = async () => {
      try {
        const response = await fetch(url, { signal: controller.signal });
        if (!response.ok) throw new Error(`HTTP ${response.status}`);
        const data: T = await response.json();
        setState({ data, loading: false, error: null });
      } catch (error) {
        if (error instanceof DOMException && error.name === 'AbortError') {
          setState({ data: null, loading: false, error: 'Request timed out. Please check your connection and try again.' });
        } else {
          setState({
            data: null,
            loading: false,
            error: sanitizeUserError(error instanceof Error ? error.message : String(error)),
          });
        }
      } finally {
        clearTimeout(timeoutId);
      }
    };

    doFetch();

    return () => {
      controller.abort();
      clearTimeout(timeoutId);
    };
  }, [url, timeout, skip]);

  return state;
}

/**
 * Build the db-api URL for a given form ID.
 */
export function getFormApiUrl(formId?: string): string | null {
  const dbApiUrl = process.env.NEXT_PUBLIC_DATABASE_API_URL || 'http://localhost:4001';
  const id = formId || process.env.NEXT_PUBLIC_FORM_ID || '';
  if (!id) return null;
  return `${dbApiUrl}/v1/forms/${encodeURIComponent(id)}`;
}

// ==================== useWallet ====================

interface UseWalletOptions {
  /** Called whenever the connected account changes (including disconnect). */
  onAccountChange?: (newAccount: string | null) => void;
}

interface UseWalletReturn {
  /** Currently connected NEAR account ID, or null. */
  account: string | null;
  /** True while the wallet selector is being initialized. */
  initializing: boolean;
  /** Open the wallet selector modal. */
  connectWallet: () => Promise<void>;
  /** Sign out of the current wallet. */
  disconnectWallet: () => Promise<void>;
}

/**
 * Manage NEAR wallet connection with cleanup on unmount.
 * Uses dynamic imports to preserve SSR safety.
 */
export function useWallet(options?: UseWalletOptions): UseWalletReturn {
  const [account, setAccount] = useState<string | null>(null);
  const [initializing, setInitializing] = useState(true);
  const unsubscribeRef = useRef<(() => void) | null>(null);
  const onAccountChangeRef = useRef(options?.onAccountChange);

  // Keep callback ref current without re-subscribing
  useEffect(() => {
    onAccountChangeRef.current = options?.onAccountChange;
  }, [options?.onAccountChange]);

  useEffect(() => {
    let cancelled = false;

    const init = async () => {
      try {
        const { initWalletSelector, getAccounts } = await import('@/lib/near');
        const selector = await initWalletSelector();

        if (cancelled) return;

        const subscription = selector.store.observable.subscribe((state) => {
          const newAccount = state.accounts.length > 0 ? state.accounts[0].accountId : null;
          setAccount(newAccount);
          onAccountChangeRef.current?.(newAccount);
        });
        unsubscribeRef.current = () => subscription.unsubscribe();

        const accounts = await getAccounts();
        if (!cancelled && accounts.length > 0) {
          setAccount(accounts[0].accountId);
        }
      } catch (error) {
        if (!cancelled) {
          console.error('Wallet init error:', error);
        }
      } finally {
        if (!cancelled) {
          setInitializing(false);
        }
      }
    };

    init();

    return () => {
      cancelled = true;
      unsubscribeRef.current?.();
    };
  }, []);

  const connectWallet = useCallback(async () => {
    const { showModal } = await import('@/lib/near');
    showModal();
  }, []);

  const disconnectWallet = useCallback(async () => {
    const { signOut } = await import('@/lib/near');
    await signOut();
    setAccount(null);
  }, []);

  return { account, initializing, connectWallet, disconnectWallet };
}
