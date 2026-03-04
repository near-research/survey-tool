import { useEffect, useState, useRef, useCallback, useMemo } from 'react';
import Head from 'next/head';
import Link from 'next/link';
import type { FormData } from '@/lib/types';
import { getFormApiUrl, useFetchWithTimeout, useWallet } from '@/lib/hooks';
import { formatAnswer, getSortedResponses, sanitizeUserError } from '@/lib/form-helpers';
import { WALLET_ERR_CANCELLED, WALLET_ERR_WINDOW_CLOSED } from '@/lib/near';
import type { ResponseRecord as Response, SortField, SortDirection } from '@/lib/form-helpers';

interface SkippedSubmission {
  submitter_id: string;
  error: string;
}

/** Decrypted payload from the WASI module (inside the EC01 envelope) */
interface ReadResponsesPayload {
  responses: Response[];
  skipped_count: number;
  skipped_submissions?: SkippedSubmission[];
  total_count: number;
  has_more: boolean;
  /** Authoritative offset for next page (optional for rolling deploy compatibility) */
  next_offset?: number;
}

const PAGE_SIZE = 50;
const MAX_LOADED_RESPONSES = 1000;

export default function ResponsesPage() {
  const [responses, setResponses] = useState<Response[]>([]);
  const [loadingResponses, setLoadingResponses] = useState(false);
  const [message, setMessage] = useState<{ type: 'success' | 'error'; text: string } | null>(null);
  const [skippedCount, setSkippedCount] = useState(0);
  const [skippedSubmissions, setSkippedSubmissions] = useState<SkippedSubmission[]>([]);
  const [totalCount, setTotalCount] = useState(0);
  const [hasMore, setHasMore] = useState(false);
  const [currentOffset, setCurrentOffset] = useState(0);
  const [sortField, setSortField] = useState<SortField>('submitted_at');
  const [sortDirection, setSortDirection] = useState<SortDirection>('desc');
  const loadGenRef = useRef(0);

  const { account, initializing, connectWallet, disconnectWallet } = useWallet({
    onAccountChange: () => {
      setMessage(null);
      loadGenRef.current += 1;
      setResponses([]);
      setHasMore(false);
      setCurrentOffset(0);
      setSkippedCount(0);
      setSkippedSubmissions([]);
      setTotalCount(0);
      setSortField('submitted_at');
      setSortDirection('desc');
    },
  });

  const formUrl = getFormApiUrl();
  const { data: form, loading: formLoading, error: formError } = useFetchWithTimeout<FormData>(formUrl);

  const loading = initializing || formLoading;

  // Derive isCreator from account and form data to avoid race condition with wallet subscriptions
  const isCreator = !!account && !!form && account === form.creator_id;

  useEffect(() => {
    if (formError) {
      setMessage({ type: 'error', text: `Failed to load form: ${formError}` });
    }
  }, [formError]);

  useEffect(() => {
    if (!initializing && account && form && account !== form.creator_id) {
      setMessage({
        type: 'error',
        text: 'Not authorized. Only the form creator can view responses.',
      });
    }
  }, [initializing, account, form]);

  const loadPage = useCallback(async (offset: number, append: boolean) => {
    if (!isCreator) {
      setMessage({ type: 'error', text: 'Not authorized to load responses' });
      return;
    }
    // Capture generation so we can detect account switches mid-flight
    const gen = loadGenRef.current;
    setMessage(null);
    if (!append) {
      setSkippedCount(0);
      setSkippedSubmissions([]);
      setTotalCount(0);
      setHasMore(false);
    }
    setLoadingResponses(true);
    // Generate ephemeral keypair — private key never leaves browser memory.
    // Declared outside try so the finally block can always zero it.
    let session: { privateKey: Uint8Array; publicKeyHex: string } | null = null;
    try {
      const { callOutLayer } = await import('@/lib/near');
      const { generateSessionKeypair, decryptEC01 } = await import('@/lib/crypto');

      session = generateSessionKeypair();

      // Call OutLayer with pagination params
      const encryptedResult = await callOutLayer('ReadResponses', {
        response_pubkey: session.publicKeyHex,
        offset,
        limit: PAGE_SIZE,
      });

      // Account changed while awaiting — discard stale results
      if (gen !== loadGenRef.current) return;

      if (!encryptedResult?.encrypted_payload) {
        setMessage({ type: 'error', text: encryptedResult?.error || 'Unexpected response: no encrypted payload' });
        return;
      }

      // Decrypt the EC01 blob using our ephemeral private key
      let payload: ReadResponsesPayload;
      try {
        const decryptedBytes = decryptEC01(session.privateKey, encryptedResult.encrypted_payload);
        // Zero the private key immediately after use, before any further processing
        session.privateKey.fill(0);
        const parsed = JSON.parse(new TextDecoder().decode(decryptedBytes));
        if (!parsed || typeof parsed !== 'object') {
          throw new Error('Invalid response payload structure');
        }
        if (!Array.isArray(parsed.responses)) {
          throw new Error('Missing or invalid "responses" field in payload');
        }
        if (typeof parsed.total_count !== 'number') {
          throw new Error('Missing or invalid "total_count" field in payload');
        }
        payload = parsed as ReadResponsesPayload;
      } catch (decryptError) {
        if (gen !== loadGenRef.current) return;
        console.error('Response decryption failed:', decryptError);
        setMessage({
          type: 'error',
          text: 'Failed to decrypt responses. Please try again.',
        });
        return;
      }

      // Account changed while decrypting — discard stale results
      if (gen !== loadGenRef.current) return;

      if (append) {
        setResponses(prev => {
          const combined = [...prev, ...(payload.responses || [])];
          return combined.length > MAX_LOADED_RESPONSES
            ? combined.slice(0, MAX_LOADED_RESPONSES)
            : combined;
        });
        setSkippedCount(prev => prev + (payload.skipped_count || 0));
        setSkippedSubmissions(prev => [...prev, ...(payload.skipped_submissions || [])]);
      } else {
        const newResponses = payload.responses || [];
        setResponses(newResponses.slice(0, MAX_LOADED_RESPONSES));
        setSkippedCount(payload.skipped_count || 0);
        setSkippedSubmissions(payload.skipped_submissions || []);
      }
      setTotalCount(payload.total_count || 0);
      setHasMore(payload.has_more || false);
      if (payload.next_offset != null) {
        setCurrentOffset(payload.next_offset);
      } else {
        console.warn('Server omitted next_offset — using fallback arithmetic');
        setCurrentOffset(offset + (payload.responses?.length || 0) + (payload.skipped_count || 0));
      }
    } catch (error) {
      if (gen !== loadGenRef.current) return;
      const msg = error instanceof Error ? error.message : String(error);
      // Silently handle user cancellation
      if (msg === WALLET_ERR_CANCELLED) {
        console.debug('User cancelled ReadResponses transaction');
        return;
      }
      // Note: Unlike forms/[id].tsx which treats Meteor wallet close as likely success
      // (the form submission transaction was likely broadcast), here we treat it as an error
      // because ReadResponses requires the actual result to decrypt and display.
      if (WALLET_ERR_WINDOW_CLOSED.some(s => s === msg)) {
        setMessage({
          type: 'error',
          text: 'Wallet closed before results were returned. The transaction may have been sent, but the decrypted responses could not be retrieved. Please try again.',
        });
        return;
      }
      console.error('Error loading responses:', error);
      setMessage({
        type: 'error',
        text: `Failed to load responses: ${sanitizeUserError(msg)}`,
      });
    } finally {
      session?.privateKey.fill(0);
      setLoadingResponses(false);
    }
  }, [isCreator]);

  const loadResponses = () => loadPage(0, false);
  const loadMore = () => loadPage(currentOffset, true);

  const toggleSort = (field: SortField) => {
    if (field === sortField) {
      // Toggle direction
      setSortDirection(sortDirection === 'asc' ? 'desc' : 'asc');
    } else {
      // Change field, default to ascending
      setSortField(field);
      setSortDirection('asc');
    }
  };

  // Sort responses based on current sort state (memoized to avoid re-sorting on unrelated renders)
  const sortedResponses = useMemo(
    () => getSortedResponses(responses, sortField, sortDirection),
    [responses, sortField, sortDirection]
  );

  if (loading) {
    return (
      <div className="min-h-screen bg-gradient-to-br from-blue-50 to-brand-100 flex items-center justify-center">
        <p className="text-gray-500 animate-pulse">Loading...</p>
      </div>
    );
  }

  if (!account) {
    return (
      <div className="min-h-screen bg-gradient-to-br from-blue-50 to-brand-100 flex items-center justify-center px-4">
        <div className="bg-white rounded-lg shadow-lg p-8 max-w-md text-center">
          <h2 className="text-2xl font-bold mb-4">Sign In Required</h2>
          <p className="text-gray-600 mb-6">
            Please sign in with your NEAR wallet to view form responses.
          </p>
          <button
            onClick={connectWallet}
            className="bg-brand-600 text-white px-6 py-2 rounded-md font-medium hover:bg-brand-700 transition"
          >
            Connect Wallet
          </button>
        </div>
      </div>
    );
  }

  if (!form) {
    return <div className="p-8 text-center text-red-600">Form not found</div>;
  }

  return (
    <>
      <Head>
        <title>Form Responses - NEAR Forms</title>
      </Head>

      <div className="min-h-screen bg-gradient-to-br from-blue-50 to-brand-100">
        <nav className="bg-white shadow-sm">
          <div className="max-w-7xl mx-auto px-4 py-4 flex justify-between items-center">
            <Link href="/" className="text-2xl font-bold text-brand-600">NEAR Forms</Link>
            <div className="flex items-center gap-4">
              <span className="text-sm text-gray-600">{account}</span>
              <button
                onClick={disconnectWallet}
                className="text-sm text-red-600 hover:text-red-800"
              >
                Disconnect
              </button>
            </div>
          </div>
        </nav>

        <div className="max-w-7xl mx-auto py-12 px-4">
          {message && (
            <div
              role="alert"
              className={`mb-4 p-4 rounded-lg ${
                message.type === 'success'
                  ? 'bg-green-100 text-green-800 border border-green-400'
                  : 'bg-red-100 text-red-800 border border-red-400'
              }`}
            >
              {message.text}
            </div>
          )}

          {skippedCount > 0 && (
            <div className="mb-4 p-4 rounded-lg bg-yellow-50 text-yellow-800 border border-yellow-300">
              <p>
                <span className="font-semibold">Warning:</span> {skippedCount} submission{skippedCount !== 1 ? 's' : ''} could not be decrypted and {skippedCount !== 1 ? 'are' : 'is'} not shown.
              </p>
              {skippedSubmissions.length > 0 && (
                <details className="mt-2">
                  <summary className="cursor-pointer text-sm underline">Show details</summary>
                  <ul className="mt-1 text-sm list-disc list-inside">
                    {skippedSubmissions.map((s, i) => (
                      <li key={i}>
                        <span className="font-mono">{s.submitter_id}</span>: {s.error}
                      </li>
                    ))}
                  </ul>
                </details>
              )}
            </div>
          )}

          <div className="bg-white rounded-lg shadow-lg">
            <div className="px-6 py-4 border-b">
              <h2 className="text-2xl font-bold mb-4">{form.title}</h2>
              <div className="flex items-center gap-4">
                <button
                  onClick={loadResponses}
                  disabled={loadingResponses || !isCreator}
                  className="bg-brand-600 text-white px-6 py-2 rounded-md font-medium hover:bg-brand-700 transition disabled:bg-gray-400"
                >
                  {loadingResponses ? 'Loading Responses...' : 'Load Responses'}
                </button>
                {responses.length > 0 && (
                  <p className="text-gray-600 text-sm">
                    Showing {responses.length} of {totalCount} submission{totalCount !== 1 ? 's' : ''}
                  </p>
                )}
              </div>
            </div>

            {responses.length === 0 ? (
              <div className="px-6 py-12 text-center text-gray-500">
                No responses yet. Click &ldquo;Load Responses&rdquo; above to fetch and decrypt submissions.
              </div>
            ) : (
              <>
                <div className="overflow-x-auto">
                  <table className="w-full">
                    <thead className="bg-gray-50 border-b">
                      <tr>
                        <th
                          scope="col"
                          role="columnheader"
                          aria-sort={sortField === 'submitter_id' ? (sortDirection === 'asc' ? 'ascending' : 'descending') : 'none'}
                          onClick={() => toggleSort('submitter_id')}
                          className="px-6 py-3 text-left text-sm font-semibold text-gray-700 min-w-[150px] cursor-pointer hover:bg-gray-100"
                          tabIndex={0}
                          onKeyDown={(e) => { if (e.key === 'Enter' || e.key === ' ') { e.preventDefault(); toggleSort('submitter_id'); } }}
                        >
                          <div className="flex items-center gap-2">
                            <span>Submitter</span>
                            {sortField === 'submitter_id' && (
                              <span aria-hidden="true">{sortDirection === 'asc' ? '\u2191' : '\u2193'}</span>
                            )}
                          </div>
                        </th>
                        <th
                          scope="col"
                          role="columnheader"
                          aria-sort={sortField === 'submitted_at' ? (sortDirection === 'asc' ? 'ascending' : 'descending') : 'none'}
                          onClick={() => toggleSort('submitted_at')}
                          className="px-6 py-3 text-left text-sm font-semibold text-gray-700 min-w-[150px] cursor-pointer hover:bg-gray-100"
                          tabIndex={0}
                          onKeyDown={(e) => { if (e.key === 'Enter' || e.key === ' ') { e.preventDefault(); toggleSort('submitted_at'); } }}
                        >
                          <div className="flex items-center gap-2">
                            <span>Submitted At</span>
                            {sortField === 'submitted_at' && (
                              <span aria-hidden="true">{sortDirection === 'asc' ? '\u2191' : '\u2193'}</span>
                            )}
                          </div>
                        </th>
                        {form.questions.map((question) => (
                          <th
                            scope="col"
                            key={question.id}
                            className="px-6 py-3 text-left text-sm font-semibold text-gray-700 min-w-[200px]"
                            title={question.label}
                          >
                            <span className="truncate block max-w-xs">
                              {question.label}
                            </span>
                          </th>
                        ))}
                      </tr>
                    </thead>
                    <tbody className="divide-y">
                      {/* Key uses submitter_id alone — unique per form via UNIQUE(form_id, submitter_id) DB constraint */}
                      {sortedResponses.map((response) => (
                        <tr key={response.submitter_id} className="hover:bg-gray-50">
                          <td className="px-6 py-4 text-sm font-mono text-gray-900">
                            {response.submitter_id}
                          </td>
                          <td className="px-6 py-4 text-sm text-gray-600 whitespace-nowrap">
                            {new Date(response.submitted_at).toLocaleString()}
                          </td>
                          {form.questions.map((question) => (
                            <td key={question.id} className="px-6 py-4 text-sm text-gray-900">
                              {formatAnswer(response.answers[question.id])}
                            </td>
                          ))}
                        </tr>
                      ))}
                    </tbody>
                  </table>
                </div>

                {hasMore && responses.length < MAX_LOADED_RESPONSES && (
                  <div className="px-6 py-4 border-t text-center">
                    <button
                      onClick={loadMore}
                      disabled={loadingResponses}
                      className="bg-gray-100 text-gray-700 px-6 py-2 rounded-md font-medium hover:bg-gray-200 transition disabled:bg-gray-50 disabled:text-gray-400"
                    >
                      {loadingResponses ? 'Loading...' : `Load More (${responses.length} of ${totalCount})`}
                    </button>
                  </div>
                )}
                {responses.length >= MAX_LOADED_RESPONSES && totalCount > MAX_LOADED_RESPONSES && (
                  <div className="px-6 py-4 border-t text-center text-sm text-gray-500">
                    Showing {MAX_LOADED_RESPONSES} of {totalCount} responses (browser limit reached).
                  </div>
                )}
              </>
            )}
          </div>
        </div>
      </div>
    </>
  );
}
