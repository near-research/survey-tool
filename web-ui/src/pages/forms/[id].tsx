import { useRouter } from 'next/router';
import { useEffect, useState, useRef } from 'react';
import Head from 'next/head';
import Link from 'next/link';

// ==================== Types ====================

interface FormQuestion {
  id: string;
  section: number;
  section_title: string;
  label: string;
  type: 'single_select' | 'multi_select' | 'rank' | 'open_text' | 'contact';
  options: string[] | null;
  optional: boolean;
  show_if: { question_id: string; value?: string; values?: string[] } | null;
}

interface FormData {
  id: string;
  title: string;
  questions: FormQuestion[];
  creator_id: string;
}

type AnswerValue = string | string[];
type AnswerMap = Record<string, AnswerValue>;

// ==================== Helper Functions ====================

/**
 * Determine if a question should be visible based on answers
 */
function isVisible(question: FormQuestion, answers: AnswerMap): boolean {
  if (!question.show_if) return true;

  const { question_id, value, values } = question.show_if;
  const answer = answers[question_id];

  // If referenced question doesn't have an answer yet, hide this question
  // (it will be revealed once the dependency is answered with the correct value)
  if (answer === undefined) return false;

  // Handle multi-value conditionals (values array)
  if (values) {
    if (Array.isArray(answer)) {
      return answer.some(a => values.includes(a));
    }
    return values.includes(answer as string);
  }

  // Handle single-value conditionals
  return answer === value;
}

/**
 * Reset answers for hidden questions and handle transitive chains
 * E.g., q12 → q12b → q13: changing q12 clears q12b and q13
 */
function resetHiddenAnswers(answers: AnswerMap, questions: FormQuestion[]): AnswerMap {
  let changed = true;
  let result = { ...answers };
  let iterations = 0;
  const MAX_ITERATIONS = questions.length + 2;

  // Loop until no more changes (handles transitive chains)
  // Cap iterations to prevent infinite loops on circular show_if dependencies
  while (changed && iterations < MAX_ITERATIONS) {
    iterations++;
    changed = false;
    for (const q of questions) {
      if (!isVisible(q, result)) {
        // Clear the answer if the question is not visible
        const currentValue = result[q.id];
        const isEmpty = currentValue === '' || currentValue === undefined || (Array.isArray(currentValue) && currentValue.length === 0);

        if (!isEmpty) {
          result = { ...result, [q.id]: Array.isArray(currentValue) ? [] : '' };
          changed = true;
        }
      }
    }
  }

  return result;
}

// ==================== Question Renderers ====================

interface QuestionProps {
  question: FormQuestion;
  value: AnswerValue;
  onChange: (questionId: string, value: AnswerValue) => void;
  isHidden: boolean;
}

function SingleSelectQuestion({ question, value, onChange, isHidden }: QuestionProps) {
  if (isHidden) return null;

  return (
    <div className="mb-6">
      <label className="block text-sm font-medium text-gray-700 mb-3">
        {question.label}
        {!question.optional && <span className="text-red-500"> *</span>}
      </label>
      <fieldset className="space-y-2">
        {question.options?.map((option) => (
          <div key={option} className="flex items-center hover:bg-gray-50 rounded px-2 py-1 -mx-2">
            <input
              type="radio"
              id={`${question.id}-${option}`}
              name={question.id}
              value={option}
              checked={value === option}
              onChange={() => onChange(question.id, option)}
              className="w-4 h-4 text-brand-600"
            />
            <label htmlFor={`${question.id}-${option}`} className="ml-3 text-sm text-gray-700">
              {option}
            </label>
          </div>
        ))}
      </fieldset>
    </div>
  );
}

function MultiSelectQuestion({ question, value, onChange, isHidden }: QuestionProps) {
  if (isHidden) return null;

  const selectedValues = Array.isArray(value) ? value : [];

  const handleChange = (option: string) => {
    const newValues = selectedValues.includes(option)
      ? selectedValues.filter(v => v !== option)
      : [...selectedValues, option];
    onChange(question.id, newValues);
  };

  return (
    <div className="mb-6">
      <label className="block text-sm font-medium text-gray-700 mb-3">
        {question.label}
        {!question.optional && <span className="text-red-500"> *</span>}
      </label>
      <fieldset className="space-y-2">
        {question.options?.map((option) => (
          <div key={option} className="flex items-center hover:bg-gray-50 rounded px-2 py-1 -mx-2">
            <input
              type="checkbox"
              id={`${question.id}-${option}`}
              checked={selectedValues.includes(option)}
              onChange={() => handleChange(option)}
              className="w-4 h-4 text-brand-600"
            />
            <label htmlFor={`${question.id}-${option}`} className="ml-3 text-sm text-gray-700">
              {option}
            </label>
          </div>
        ))}
      </fieldset>
    </div>
  );
}

function RankQuestion({ question, value, onChange, isHidden }: QuestionProps) {
  if (isHidden) return null;

  const selectedValues = Array.isArray(value) ? value : [];
  const unselectedOptions = question.options?.filter(opt => !selectedValues.includes(opt)) || [];
  const ranks = ['1st choice', '2nd choice', '3rd choice'];

  const handleSelectRank = (rankIndex: number, option: string) => {
    const newValues = Array.from({ length: 3 }, (_, i) => selectedValues[i] ?? '');
    // Remove this option from any other rank (prevent duplicates)
    for (let i = 0; i < newValues.length; i++) {
      if (i !== rankIndex && newValues[i] === option) newValues[i] = '';
    }
    newValues[rankIndex] = option;
    onChange(question.id, newValues);
  };

  const handleClearRank = (rankIndex: number) => {
    const newValues = Array.from({ length: 3 }, (_, i) => selectedValues[i] ?? '');
    newValues[rankIndex] = '';
    onChange(question.id, newValues);
  };

  return (
    <div className="mb-6">
      <label className="block text-sm font-medium text-gray-700 mb-3">
        {question.label}
        {!question.optional && <span className="text-red-500"> *</span>}
      </label>
      <div className="space-y-3">
        {ranks.map((rankLabel, rankIndex) => (
          <div key={rankIndex} className="flex items-center gap-3">
            <span className="text-sm font-medium text-gray-600 w-20">{rankLabel}</span>
            <select
              value={selectedValues[rankIndex] || ''}
              onChange={(e) => handleSelectRank(rankIndex, e.target.value)}
              className="flex-1 px-3 py-2 border border-gray-300 rounded-md focus:outline-none focus:ring-2 focus:ring-brand-500"
            >
              <option value="">-- Select --</option>
              {unselectedOptions
                .concat([selectedValues[rankIndex]].filter(Boolean) as string[])
                .filter((opt, idx, arr) => arr.indexOf(opt) === idx) // unique
                .map((opt) => (
                  <option key={opt} value={opt}>
                    {opt}
                  </option>
                ))}
            </select>
            {selectedValues[rankIndex] && (
              <button
                type="button"
                onClick={() => handleClearRank(rankIndex)}
                className="text-red-600 hover:text-red-800 text-sm border border-red-200 rounded px-2 py-0.5 hover:bg-red-50 transition"
              >
                Clear
              </button>
            )}
          </div>
        ))}
      </div>
    </div>
  );
}

function OpenTextQuestion({ question, value, onChange, isHidden }: QuestionProps) {
  if (isHidden) return null;

  return (
    <div className="mb-6">
      <label htmlFor={question.id} className="block text-sm font-medium text-gray-700 mb-2">
        {question.label}
        {!question.optional && <span className="text-red-500"> *</span>}
      </label>
      <textarea
        id={question.id}
        value={typeof value === 'string' ? value : ''}
        onChange={(e) => onChange(question.id, e.target.value)}
        maxLength={5000}
        className="w-full px-3 py-2 border border-gray-300 rounded-md focus:outline-none focus:ring-2 focus:ring-brand-500"
        rows={4}
      />
      <p className="mt-1 text-xs text-gray-400 text-right">
        {(typeof value === 'string' ? value : '').length}/5000
      </p>
    </div>
  );
}

function ContactQuestion({ question, value, onChange, isHidden }: QuestionProps) {
  if (isHidden) return null;

  const selectedOption = typeof value === 'string' ? value.split(':')[0] : '';
  const contactDetail = typeof value === 'string' && value.includes(':')
    ? value.split(':').slice(1).join(':')
    : '';
  const showInput = selectedOption.startsWith('Yes');

  return (
    <div className="mb-6">
      <label className="block text-sm font-medium text-gray-700 mb-3">
        {question.label}
        {!question.optional && <span className="text-red-500"> *</span>}
      </label>
      <fieldset className="space-y-3">
        {question.options?.map((option) => (
          <div key={option}>
            <div className="flex items-center hover:bg-gray-50 rounded px-2 py-1 -mx-2">
              <input
                type="radio"
                id={`${question.id}-${option}`}
                name={question.id}
                value={option}
                checked={selectedOption === option}
                onChange={() => onChange(question.id, option)}
                className="w-4 h-4 text-brand-600"
              />
              <label htmlFor={`${question.id}-${option}`} className="ml-3 text-sm text-gray-700">
                {option}
              </label>
            </div>
            {showInput && option === selectedOption && (
              <input
                type="text"
                value={contactDetail}
                placeholder="Your email or handle..."
                maxLength={200}
                className="mt-2 ml-7 px-3 py-2 border border-gray-300 rounded-md focus:outline-none focus:ring-2 focus:ring-brand-500"
                onChange={(e) => onChange(question.id, `${option}:${e.target.value}`)}
              />
            )}
          </div>
        ))}
      </fieldset>
    </div>
  );
}

function QuestionRenderer({ question, value, onChange, isHidden }: QuestionProps) {
  switch (question.type) {
    case 'single_select':
      return <SingleSelectQuestion question={question} value={value} onChange={onChange} isHidden={isHidden} />;
    case 'multi_select':
      return <MultiSelectQuestion question={question} value={value} onChange={onChange} isHidden={isHidden} />;
    case 'rank':
      return <RankQuestion question={question} value={value} onChange={onChange} isHidden={isHidden} />;
    case 'open_text':
      return <OpenTextQuestion question={question} value={value} onChange={onChange} isHidden={isHidden} />;
    case 'contact':
      return <ContactQuestion question={question} value={value} onChange={onChange} isHidden={isHidden} />;
    default:
      return null;
  }
}

// ==================== Page Component ====================

export default function SubmitFormPage() {
  const router = useRouter();
  const { id: formId } = router.query;

  const [form, setForm] = useState<FormData | null>(null);
  const [answers, setAnswers] = useState<AnswerMap>({});
  const [account, setAccount] = useState<string | null>(null);
  const [loading, setLoading] = useState(true);
  const [submitting, setSubmitting] = useState(false);
  const [message, setMessage] = useState<{ type: 'success' | 'error'; text: string } | null>(null);
  const unsubscribeRef = useRef<(() => void) | null>(null);

  // Initialize wallet
  useEffect(() => {
    let cancelled = false;

    const initWallet = async () => {
      try {
        const { initWalletSelector, getAccounts } = await import('@/lib/near');
        const selector = await initWalletSelector();

        if (cancelled) return;

        // Subscribe to wallet state changes with cleanup
        const subscription = selector.store.observable.subscribe((state) => {
          if (state.accounts.length > 0) {
            setAccount(state.accounts[0].accountId);
          } else {
            setAccount(null);
          }
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
      }
    };

    initWallet();

    return () => {
      cancelled = true;
      unsubscribeRef.current?.();
    };
  }, []);

  // Load form metadata
  useEffect(() => {
    const loadForm = async () => {
      if (!formId) return;
      try {
        const dbApiUrl = process.env.NEXT_PUBLIC_DATABASE_API_URL || 'http://localhost:4001';
        // Use URL param first, fall back to env var for development convenience
        const formId_ = formId || process.env.NEXT_PUBLIC_FORM_ID;
        const response = await fetch(`${dbApiUrl}/forms/${formId_}`);
        if (!response.ok) throw new Error('Failed to load form');
        const data = await response.json();
        setForm(data);

        // Initialize empty answers
        const initialAnswers: AnswerMap = {};
        data.questions.forEach((q: FormQuestion) => {
          if (q.type === 'multi_select' || q.type === 'rank') {
            initialAnswers[q.id] = [];
          } else {
            initialAnswers[q.id] = '';
          }
        });
        setAnswers(initialAnswers);
      } catch (error) {
        setMessage({ type: 'error', text: `Failed to load form: ${error}` });
      } finally {
        setLoading(false);
      }
    };

    loadForm();
  }, [formId]);

  const handleAnswerChange = (questionId: string, value: AnswerValue) => {
    const newAnswers = { ...answers, [questionId]: value };
    const resetAnswers = form ? resetHiddenAnswers(newAnswers, form.questions) : newAnswers;
    setAnswers(resetAnswers);
  };

  const handleSubmit = async (e: React.FormEvent) => {
    e.preventDefault();

    if (submitting) return;

    if (!account) {
      setMessage({ type: 'error', text: 'Please sign in with your NEAR wallet' });
      return;
    }

    if (!form) {
      setMessage({ type: 'error', text: 'Form not loaded' });
      return;
    }

    // Client-side validation of required fields
    const missing = form.questions
      .filter(q => isVisible(q, answers) && !q.optional)
      .filter(q => {
        const v = answers[q.id];
        return v === '' || v === undefined ||
          (Array.isArray(v) && (v.length === 0 || v.every((s: string) => s === '')));
      });
    if (missing.length > 0) {
      setMessage({
        type: 'error',
        text: `Please fill in required fields: ${missing.map(q => q.label).join(', ')}`,
      });
      return;
    }

    // Validate contact fields: if "Yes" is selected, contact detail must be non-empty
    const invalidContacts = form.questions
      .filter(q => q.type === 'contact' && isVisible(q, answers))
      .filter(q => {
        const v = answers[q.id];
        return typeof v === 'string' && v.startsWith('Yes') && v.includes(':') && v.split(':').slice(1).join(':').trim() === '';
      });
    if (invalidContacts.length > 0) {
      setMessage({
        type: 'error',
        text: `Please provide contact details for: ${invalidContacts.map(q => q.label).join(', ')}`,
      });
      return;
    }

    setSubmitting(true);

    try {
      const { callOutLayer } = await import('@/lib/near');
      const { encryptFormAnswers } = await import('@/lib/crypto');

      // Filter to only visible questions before sending
      const visibleAnswers = Object.fromEntries(
        form.questions
          .filter(q => isVisible(q, answers))
          .map(q => [q.id, answers[q.id]])
      );

      // Encrypt answers client-side so plaintext never appears on-chain
      const masterPubKey = process.env.NEXT_PUBLIC_MASTER_PUBLIC_KEY;
      if (!masterPubKey) {
        throw new Error('NEXT_PUBLIC_MASTER_PUBLIC_KEY not configured');
      }
      if (!/^0[23][0-9a-fA-F]{64}$/.test(masterPubKey)) {
        throw new Error('NEXT_PUBLIC_MASTER_PUBLIC_KEY must be a 66-char hex compressed secp256k1 public key (starts with 02 or 03)');
      }
      const encryptedAnswers = encryptFormAnswers(masterPubKey, form.id, visibleAnswers);

      // Send encrypted blob instead of plaintext answers
      const result = await callOutLayer('SubmitForm', { encrypted_answers: encryptedAnswers });

      // Ensure result is a valid object before accessing properties
      if (!result || typeof result !== 'object') {
        throw new Error(`Unexpected response from OutLayer: ${JSON.stringify(result)}`);
      }

      if (result.success === false) {
        throw new Error((result as any).error || 'Submission failed');
      }

      setMessage({ type: 'success', text: 'Form submitted successfully!' });
      setSubmitting(false);
    } catch (error) {
      const msg = error instanceof Error ? error.message : String(error);
      // Silently handle user cancellation
      if (msg === 'Transaction cancelled') {
        setSubmitting(false);
        return;
      }
      // Meteor wallet closes popup after broadcasting, returning "User closed the window"
      // even though the transaction was sent. Show informational message instead of error.
      if (msg.toLowerCase().includes('closed')) {
        setMessage({
          type: 'success',
          text: 'Your transaction was sent. Please check your wallet or NEAR Explorer to confirm.',
        });
        setSubmitting(false);
        return;
      }
      setMessage({
        type: 'error',
        text: `Submission failed: ${msg}`,
      });
      setSubmitting(false);
    }
  };

  if (loading) {
    return (
      <div className="min-h-screen bg-gradient-to-br from-blue-50 to-brand-100 flex items-center justify-center">
        <p className="text-gray-500 animate-pulse">Loading form...</p>
      </div>
    );
  }

  if (!form) {
    return (
      <div className="min-h-screen bg-gradient-to-br from-blue-50 to-brand-100 flex items-center justify-center">
        <p className="text-red-600">Form not found</p>
      </div>
    );
  }

  // Group questions by section, sorted numerically
  const sections = Array.from(new Set(form.questions.map(q => q.section))).sort((a, b) => a - b);
  const questionsBySection: Record<number, FormQuestion[]> = {};
  sections.forEach(section => {
    questionsBySection[section] = form.questions.filter(q => q.section === section);
  });

  return (
    <>
      <Head>
        <title>{form.title} - NEAR Forms</title>
      </Head>

      <div className="min-h-screen bg-gradient-to-br from-blue-50 to-brand-100">
        <nav className="bg-white shadow-sm">
          <div className="max-w-4xl mx-auto px-4 py-4 flex justify-between items-center">
            <Link href="/" className="text-2xl font-bold text-brand-600">NEAR Forms</Link>
            <Link href="/responses" className="text-sm text-brand-600 hover:text-brand-800">
              Responses
            </Link>
          </div>
        </nav>

        <div className="py-12 px-4">
        <div className="max-w-2xl mx-auto">
          <div className="bg-white rounded-lg shadow-lg p-8">
            <h1 className="text-3xl font-bold mb-2">{form.title}</h1>

            {!account ? (
              <div className="mb-8">
                <p className="text-gray-600 mb-4">
                  Please sign in with your NEAR wallet to submit this form.
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
            ) : (
              <div className="flex items-center gap-4 mb-8">
                <p className="text-gray-600">
                  Signed in as: <span className="font-semibold">{account}</span>
                </p>
                <button
                  onClick={async () => {
                    const { signOut } = await import('@/lib/near');
                    await signOut();
                    setAccount(null);
                  }}
                  className="text-sm text-red-600 hover:text-red-800"
                >
                  Disconnect
                </button>
              </div>
            )}

            {message && (
              <div
                className={`mb-6 p-4 rounded-lg ${
                  message.type === 'success'
                    ? 'bg-green-100 text-green-800 border border-green-400'
                    : 'bg-red-100 text-red-800 border border-red-400'
                }`}
              >
                {message.text}
                {message.type === 'success' && (
                  <Link href="/" className="ml-2 underline font-medium hover:text-green-900">
                    Return to Home
                  </Link>
                )}
              </div>
            )}

            <form onSubmit={handleSubmit} className="space-y-8">
              {sections.map((section) => (
                <div key={section}>
                  <h2 className="text-lg font-semibold text-gray-800 mb-6 pb-3 border-b-2 border-brand-200">
                    {questionsBySection[section][0]?.section_title}
                  </h2>

                  <div className="space-y-6">
                    {questionsBySection[section].map((question) => (
                      <QuestionRenderer
                        key={question.id}
                        question={question}
                        value={answers[question.id] || (question.type === 'multi_select' || question.type === 'rank' ? [] : '')}
                        onChange={handleAnswerChange}
                        isHidden={!isVisible(question, answers)}
                      />
                    ))}
                  </div>
                </div>
              ))}

              <button
                type="submit"
                disabled={!account || submitting}
                className="w-full bg-brand-600 text-white py-3 rounded-md font-medium hover:bg-brand-700 disabled:bg-gray-400 disabled:cursor-not-allowed transition"
              >
                {submitting ? 'Submitting...' : 'Submit Form'}
              </button>
            </form>
          </div>
        </div>
        </div>
      </div>
    </>
  );
}
