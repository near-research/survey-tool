import { useEffect, useState, useRef } from 'react';
import { useRouter } from 'next/router';
import Head from 'next/head';
import Link from 'next/link';

interface FormQuestion {
  id: string;
  section: number;
  section_title: string;
  label: string;
  type: string;
  options: string[] | null;
  optional: boolean;
  show_if: any;
}

interface FormData {
  id: string;
  title: string;
  questions: FormQuestion[];
  creator_id: string;
}

interface Response {
  submitter_id: string;
  answers: Record<string, any>;
  submitted_at: string;
}

function formatAnswer(value: any): string {
  if (value === undefined || value === null || value === '') {
    return '—';
  }

  if (Array.isArray(value)) {
    return value.join(' · ');
  }

  return String(value);
}

function getSortedResponses(
  responses: Response[],
  field: SortField,
  direction: SortDirection
): Response[] {
  const sorted = [...responses];
  sorted.sort((a, b) => {
    let aVal: string | number;
    let bVal: string | number;

    if (field === 'submitter_id') {
      aVal = a.submitter_id;
      bVal = b.submitter_id;
    } else {
      // submitted_at - sort as dates
      aVal = new Date(a.submitted_at).getTime();
      bVal = new Date(b.submitted_at).getTime();
    }

    if (aVal < bVal) return direction === 'asc' ? -1 : 1;
    if (aVal > bVal) return direction === 'asc' ? 1 : -1;
    return 0;
  });

  return sorted;
}

type SortField = 'submitter_id' | 'submitted_at';
type SortDirection = 'asc' | 'desc';

export default function ResponsesPage() {
  const router = useRouter();
  const [form, setForm] = useState<FormData | null>(null);
  const [responses, setResponses] = useState<Response[]>([]);
  const [account, setAccount] = useState<string | null>(null);
  const [loading, setLoading] = useState(true);
  const [loadingResponses, setLoadingResponses] = useState(false);
  const [message, setMessage] = useState<{ type: 'success' | 'error'; text: string } | null>(null);
  const [skippedCount, setSkippedCount] = useState(0);
  const [sortField, setSortField] = useState<SortField>('submitted_at');
  const [sortDirection, setSortDirection] = useState<SortDirection>('desc');
  const unsubscribeRef = useRef<(() => void) | null>(null);

  // Derive isCreator from account and form data to avoid race condition with wallet subscriptions
  const isCreator = !!account && !!form && account === form.creator_id;

  useEffect(() => {
    let cancelled = false;

    const init = async () => {
      try {
        const { initWalletSelector, getAccounts } = await import('@/lib/near');

        // Initialize wallet
        const selector = await initWalletSelector();

        if (cancelled) return;

        const accounts = await getAccounts();

        if (cancelled) return;

        // Subscribe to wallet state changes
        const subscription = selector.store.observable.subscribe((state) => {
          if (state.accounts.length > 0) {
            setAccount(state.accounts[0].accountId);
            setMessage(null); // clear stale error on new account
          } else {
            setAccount(null);
          }
        });
        unsubscribeRef.current = () => subscription.unsubscribe();

        if (accounts.length === 0) {
          setLoading(false);
          return;
        }

        const currentAccount = accounts[0].accountId;
        setAccount(currentAccount);

        // Fetch form metadata first to check authorization
        const dbApiUrl = process.env.NEXT_PUBLIC_DATABASE_API_URL || 'http://localhost:4001';
        const formId = (router.query.form_id as string) || process.env.NEXT_PUBLIC_FORM_ID || '';
        if (!formId) {
          throw new Error('Form ID not specified. Set NEXT_PUBLIC_FORM_ID or use ?form_id= query parameter.');
        }

        const formResponse = await fetch(`${dbApiUrl}/forms/${formId}`);
        if (!formResponse.ok) {
          throw new Error('Failed to load form');
        }
        const formData = await formResponse.json();

        if (cancelled) return;

        setForm(formData);

        // Check if current account is the form creator
        if (currentAccount !== formData.creator_id) {
          setMessage({
            type: 'error',
            text: `Not authorized. Only the form creator can view responses.`,
          });
          setLoading(false);
          return;
        }
      } catch (error) {
        if (!cancelled) {
          console.error('Error loading form:', error);
          setMessage({
            type: 'error',
            text: `Failed to load form: ${error instanceof Error ? error.message : String(error)}`,
          });
        }
      } finally {
        if (!cancelled) {
          setLoading(false);
        }
      }
    };

    init();

    return () => {
      cancelled = true;
      unsubscribeRef.current?.();
    };
  }, [router.query.form_id]);

  const loadResponses = async () => {
    if (!isCreator) {
      setMessage({ type: 'error', text: 'Not authorized to load responses' });
      return;
    }
    setMessage(null);
    setSkippedCount(0);
    setLoadingResponses(true);
    try {
      const { callOutLayer } = await import('@/lib/near');

      // Call OutLayer to fetch and decrypt responses
      const readResponsesOutput = await callOutLayer('ReadResponses', {});

      if (readResponsesOutput?.success === false || readResponsesOutput?.error) {
        setMessage({ type: 'error', text: readResponsesOutput?.error || 'Failed to read responses' });
      } else if (Array.isArray(readResponsesOutput?.responses)) {
        setResponses(readResponsesOutput.responses);
        // Track skipped submissions that could not be decrypted
        const skipped = readResponsesOutput?.skipped_count || 0;
        setSkippedCount(skipped);
      }
    } catch (error) {
      const msg = error instanceof Error ? error.message : String(error);
      // Silently handle user cancellation
      if (msg === 'Transaction cancelled') {
        console.log('User cancelled ReadResponses transaction');
        return;
      }
      // Meteor wallet closes popup after broadcasting, returning "User closed the window"
      // even though the transaction was sent. Show informational message instead of error.
      if (msg.toLowerCase().includes('closed')) {
        setMessage({
          type: 'error',
          text: 'Wallet closed before results were returned. Please try again.',
        });
        return;
      }
      console.error('Error loading responses:', error);
      setMessage({
        type: 'error',
        text: `Failed to load responses: ${msg}`,
      });
    } finally {
      setLoadingResponses(false);
    }
  };

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

  // Sort responses based on current sort state
  const sortedResponses = getSortedResponses(responses, sortField, sortDirection);

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
            onClick={async () => {
              const { showModal } = await import('@/lib/near');
              showModal();
            }}
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
                onClick={async () => {
                  const { signOut } = await import('@/lib/near');
                  await signOut();
                  setAccount(null);
                  setResponses([]);
                  setMessage(null);
                }}
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
              <span className="font-semibold">⚠️ Data Loss Warning:</span> {skippedCount} submission{skippedCount !== 1 ? 's' : ''} could not be decrypted and {skippedCount !== 1 ? 'are' : 'is'} not shown above.
            </div>
          )}

          <div className="bg-white rounded-lg shadow-lg">
            <div className="px-6 py-4 border-b">
              <h2 className="text-2xl font-bold mb-4">{form.title}</h2>
              <button
                onClick={loadResponses}
                disabled={loadingResponses || !isCreator}
                className="bg-brand-600 text-white px-6 py-2 rounded-md font-medium hover:bg-brand-700 transition disabled:bg-gray-400"
              >
                {loadingResponses ? 'Loading Responses...' : 'Load Responses'}
              </button>
              {responses.length > 0 && (
                <p className="text-gray-600 text-sm mt-4">
                  {responses.length} submission{responses.length !== 1 ? 's' : ''}
                </p>
              )}
            </div>

            {responses.length === 0 ? (
              <div className="px-6 py-12 text-center text-gray-500">
                No responses yet. Click &ldquo;Load Responses&rdquo; above to fetch and decrypt submissions.
              </div>
            ) : (
              <div className="overflow-x-auto">
                <table className="w-full">
                  <thead className="bg-gray-50 border-b">
                    <tr>
                      <th
                        onClick={() => toggleSort('submitter_id')}
                        className="px-6 py-3 text-left text-sm font-semibold text-gray-700 min-w-[150px] cursor-pointer hover:bg-gray-100"
                      >
                        <div className="flex items-center gap-2">
                          <span>Submitter</span>
                          {sortField === 'submitter_id' && (
                            <span>{sortDirection === 'asc' ? '↑' : '↓'}</span>
                          )}
                        </div>
                      </th>
                      <th
                        onClick={() => toggleSort('submitted_at')}
                        className="px-6 py-3 text-left text-sm font-semibold text-gray-700 min-w-[150px] cursor-pointer hover:bg-gray-100"
                      >
                        <div className="flex items-center gap-2">
                          <span>Submitted At</span>
                          {sortField === 'submitted_at' && (
                            <span>{sortDirection === 'asc' ? '↑' : '↓'}</span>
                          )}
                        </div>
                      </th>
                      {form.questions.map((question) => (
                        <th
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
                    {sortedResponses.map((response, index) => (
                      <tr key={`${response.submitter_id}-${response.submitted_at}-${index}`} className="hover:bg-gray-50">
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
            )}
          </div>
        </div>
      </div>
    </>
  );
}
