import { useRouter } from 'next/router';
import { useEffect, useState } from 'react';
import Head from 'next/head';
import Link from 'next/link';
import type { FormQuestion, FormData } from '@/lib/types';
import { useFetchWithTimeout, getFormApiUrl, useWallet } from '@/lib/hooks';
import { deriveFormPublicKey, COMPRESSED_PUBKEY_REGEX } from '@/lib/crypto';
import { isVisible, resetHiddenAnswers, applyExclusiveMultiSelect, sanitizeUserError } from '@/lib/form-helpers';
import { WALLET_ERR_CANCELLED, WALLET_ERR_WINDOW_CLOSED } from '@/lib/near';
import type { AnswerValue, AnswerMap } from '@/lib/form-helpers';

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
    <fieldset className="mb-6">
      <legend className="block text-sm font-medium text-gray-700 mb-3">
        {question.label}
        {!question.optional && <span className="text-red-500"> *</span>}
      </legend>
      <div className="space-y-2">
        {question.options?.map((option) => (
          <div key={option} className="flex items-center hover:bg-gray-50 rounded px-2 py-1 -mx-2">
            <input
              type="radio"
              id={`${question.id}-${option}`}
              name={question.id}
              value={option}
              checked={value === option}
              onChange={() => onChange(question.id, option)}
              aria-required={!question.optional}
              className="w-4 h-4 text-brand-600"
            />
            <label htmlFor={`${question.id}-${option}`} className="ml-3 text-sm text-gray-700">
              {option}
            </label>
          </div>
        ))}
      </div>
    </fieldset>
  );
}

function MultiSelectQuestion({ question, value, onChange, isHidden }: QuestionProps) {
  if (isHidden) return null;

  const selectedValues = Array.isArray(value) ? value : [];
  const exclusiveOpts = question.exclusive_options || [];

  const handleChange = (option: string) => {
    onChange(question.id, applyExclusiveMultiSelect(selectedValues, option, exclusiveOpts));
  };

  return (
    <fieldset className="mb-6">
      <legend className="block text-sm font-medium text-gray-700 mb-3">
        {question.label}
        {!question.optional && <span className="text-red-500"> *</span>}
      </legend>
      <div className="space-y-2">
        {question.options?.map((option, index) => {
          const exclusiveSet = new Set(exclusiveOpts);
          const isExclusive = exclusiveSet.has(option);
          const showSeparator = isExclusive &&
            index > 0 &&
            !exclusiveSet.has(question.options![index - 1]);

          return (
            <div key={option}>
              {showSeparator && <hr className="my-2 border-gray-200" />}
              <div className="flex items-center hover:bg-gray-50 rounded px-2 py-1 -mx-2">
                <input
                  type="checkbox"
                  id={`${question.id}-${option}`}
                  checked={selectedValues.includes(option)}
                  onChange={() => handleChange(option)}
                  aria-required={!question.optional}
                  className="w-4 h-4 text-brand-600"
                />
                <label
                  htmlFor={`${question.id}-${option}`}
                  className={`ml-3 text-sm ${isExclusive ? 'text-gray-500 italic' : 'text-gray-700'}`}
                >
                  {option}
                </label>
              </div>
            </div>
          );
        })}
      </div>
    </fieldset>
  );
}

function RankQuestion({ question, value, onChange, isHidden }: QuestionProps) {
  if (isHidden) return null;

  const selectedValues = Array.isArray(value) ? value : [];
  const unselectedOptions = question.options?.filter(opt => !selectedValues.includes(opt)) || [];
  const rankCount = question.rank_count ?? 3;
  const ordinals = ['1st', '2nd', '3rd', '4th', '5th', '6th', '7th', '8th', '9th', '10th'];
  const ranks = Array.from({ length: rankCount }, (_, i) => `${ordinals[i] ?? `${i + 1}th`} choice`);

  const handleSelectRank = (rankIndex: number, option: string) => {
    const newValues = Array.from({ length: rankCount }, (_, i) => selectedValues[i] ?? '');
    // Remove this option from any other rank (prevent duplicates)
    for (let i = 0; i < newValues.length; i++) {
      if (i !== rankIndex && newValues[i] === option) newValues[i] = '';
    }
    newValues[rankIndex] = option;
    onChange(question.id, newValues);
  };

  const handleClearRank = (rankIndex: number) => {
    const newValues = Array.from({ length: rankCount }, (_, i) => selectedValues[i] ?? '');
    newValues[rankIndex] = '';
    onChange(question.id, newValues);
  };

  return (
    <fieldset className="mb-6">
      <legend className="block text-sm font-medium text-gray-700 mb-3">
        {question.label}
        {!question.optional && <span className="text-red-500"> *</span>}
      </legend>
      <div className="space-y-3">
        {ranks.map((rankLabel, rankIndex) => (
          <div key={rankIndex} className="flex items-center gap-3">
            <span className="text-sm font-medium text-gray-600 w-20">{rankLabel}</span>
            <select
              value={selectedValues[rankIndex] || ''}
              onChange={(e) => handleSelectRank(rankIndex, e.target.value)}
              aria-label={`${rankLabel} selection`}
              aria-required={!question.optional}
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
    </fieldset>
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
        aria-required={!question.optional}
        autoComplete="off"
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

  const handleOptionChange = (option: string) => {
    // Only preserve ":detail" suffix for "Yes" options that show the text input.
    // Non-"Yes" options (e.g., "No") store just the option name, clearing any stale detail.
    if (option.startsWith('Yes')) {
      onChange(question.id, `${option}:`);
    } else {
      onChange(question.id, option);
    }
  };

  return (
    <fieldset className="mb-6">
      <legend className="block text-sm font-medium text-gray-700 mb-3">
        {question.label}
        {!question.optional && <span className="text-red-500"> *</span>}
      </legend>
      <div className="space-y-3">
        {question.options?.map((option) => (
          <div key={option}>
            <div className="flex items-center hover:bg-gray-50 rounded px-2 py-1 -mx-2">
              <input
                type="radio"
                id={`${question.id}-${option}`}
                name={question.id}
                value={option}
                checked={selectedOption === option}
                onChange={() => handleOptionChange(option)}
                aria-required={!question.optional}
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
                aria-label="Contact details"
                autoComplete="off"
                placeholder="Your email or handle..."
                maxLength={200}
                className="mt-2 ml-7 px-3 py-2 border border-gray-300 rounded-md focus:outline-none focus:ring-2 focus:ring-brand-500"
                onChange={(e) => onChange(question.id, `${option}:${e.target.value}`)}
              />
            )}
          </div>
        ))}
      </div>
    </fieldset>
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
  const rawId = router.query.id;
  // router.query.id can be string | string[] | undefined; normalize to string
  const formId = typeof rawId === 'string' ? rawId : undefined;

  const formUrl = getFormApiUrl(formId);
  const { data: formData, loading: formLoading, error: formError } = useFetchWithTimeout<FormData>(
    formUrl,
    { skip: !formId }
  );

  const [form, setForm] = useState<FormData | null>(null);
  const [answers, setAnswers] = useState<AnswerMap>({});
  const [loading, setLoading] = useState(true);
  const [submitting, setSubmitting] = useState(false);
  const [formDisabled, setFormDisabled] = useState(false);
  const [message, setMessage] = useState<{ type: 'success' | 'error'; text: string } | null>(null);

  const { account, connectWallet, disconnectWallet } = useWallet({
    onAccountChange: () => { setMessage(null); setSubmitting(false); },
  });

  // Process form data when the fetch hook returns
  useEffect(() => {
    setFormDisabled(false);
    setMessage(null);

    if (formLoading) return;

    if (formError) {
      setMessage({ type: 'error', text: formError });
      setLoading(false);
      return;
    }

    if (formData) {
      setForm(formData);

      // Validate master public key early so users see misconfiguration immediately
      const masterPubKey = process.env.NEXT_PUBLIC_MASTER_PUBLIC_KEY;
      if (!masterPubKey) {
        setMessage({ type: 'error', text: 'NEXT_PUBLIC_MASTER_PUBLIC_KEY not configured. Form submissions are disabled.' });
        setFormDisabled(true);
      } else if (!COMPRESSED_PUBKEY_REGEX.test(masterPubKey)) {
        setMessage({ type: 'error', text: 'NEXT_PUBLIC_MASTER_PUBLIC_KEY is malformed. Expected a 66-char hex compressed secp256k1 public key (starts with 02 or 03).' });
        setFormDisabled(true);
      } else {
        // Validate the key is actually on the secp256k1 curve (not just valid hex).
        // Uses module-level import (synchronous) to avoid race window where form
        // could be submitted before async validation completes.
        try {
          deriveFormPublicKey(masterPubKey, formData.id);
        } catch {
          setMessage({ type: 'error', text: 'NEXT_PUBLIC_MASTER_PUBLIC_KEY is not a valid secp256k1 public key.' });
          setFormDisabled(true);
        }
      }

      // Initialize empty answers
      const initialAnswers: AnswerMap = {};
      formData.questions.forEach((q: FormQuestion) => {
        if (q.type === 'multi_select' || q.type === 'rank') {
          initialAnswers[q.id] = [];
        } else {
          initialAnswers[q.id] = '';
        }
      });
      setAnswers(initialAnswers);
    }

    setLoading(false);
  }, [formData, formLoading, formError]);

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
      if (!COMPRESSED_PUBKEY_REGEX.test(masterPubKey)) {
        throw new Error('NEXT_PUBLIC_MASTER_PUBLIC_KEY must be a 66-char hex compressed secp256k1 public key (starts with 02 or 03)');
      }
      const encryptedAnswers = encryptFormAnswers(masterPubKey, form.id, visibleAnswers);

      // Send encrypted blob instead of plaintext answers
      const result = await callOutLayer('SubmitForm', { encrypted_answers: encryptedAnswers });

      // Ensure result is a valid object before accessing properties
      if (!result || typeof result !== 'object') {
        console.error('Unexpected OutLayer response:', result);
        throw new Error('Unexpected response from OutLayer. Please try again.');
      }

      if (result.success !== true) {
        throw new Error(result.error || 'Submission failed');
      }

      setMessage({ type: 'success', text: 'Form submitted successfully!' });
    } catch (error) {
      const msg = error instanceof Error ? error.message : String(error);
      // Silently handle user cancellation
      if (msg === WALLET_ERR_CANCELLED) {
        return;
      }
      // Meteor wallet closes popup after broadcasting, but may also close before broadcast.
      // We can't distinguish, so treat as ambiguous — not a confirmed success or failure.
      if (WALLET_ERR_WINDOW_CLOSED.some(s => s === msg)) {
        setMessage({
          type: 'error',
          text: 'Wallet closed before confirmation. Your submission may or may not have been sent — please check your wallet or NEAR Explorer.',
        });
        return;
      }
      // Sanitize internal errors to user-friendly messages
      console.error('Submission error:', msg);
      const userMsg = sanitizeUserError(msg);
      setMessage({
        type: 'error',
        text: `Submission failed: ${userMsg}`,
      });
    } finally {
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
              <div className="text-center py-12">
                <svg className="mx-auto h-16 w-16 text-brand-400 mb-6" fill="none" viewBox="0 0 24 24" strokeWidth={1.5} stroke="currentColor">
                  <path strokeLinecap="round" strokeLinejoin="round" d="M21 12a2.25 2.25 0 0 0-2.25-2.25H15a3 3 0 1 1-6 0H5.25A2.25 2.25 0 0 0 3 12m18 0v6a2.25 2.25 0 0 1-2.25 2.25H5.25A2.25 2.25 0 0 1 3 18v-6m18 0V9M3 12V9m18 0a2.25 2.25 0 0 0-2.25-2.25H5.25A2.25 2.25 0 0 0 3 9m18 0V6a2.25 2.25 0 0 0-2.25-2.25H5.25A2.25 2.25 0 0 0 3 6v3" />
                </svg>
                <h2 className="text-xl font-semibold text-gray-800 mb-2">
                  Connect your NEAR wallet to continue
                </h2>
                <p className="text-gray-500 mb-8 max-w-md mx-auto">
                  This form requires wallet authentication. Your responses will be encrypted and can only be read by the form creator.
                </p>
                <button
                  onClick={connectWallet}
                  className="bg-brand-600 text-white px-8 py-3 rounded-md font-medium hover:bg-brand-700 transition text-lg"
                >
                  Connect Wallet
                </button>
              </div>
            ) : (
              <>
                <div className="flex items-center gap-4 mb-8">
                  <p className="text-gray-600">
                    Signed in as: <span className="font-semibold">{account}</span>
                  </p>
                  <button
                    onClick={disconnectWallet}
                    className="text-sm text-red-600 hover:text-red-800"
                  >
                    Disconnect
                  </button>
                </div>

                {message && (
                  <div
                    role="alert"
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

                <form onSubmit={handleSubmit} autoComplete="off" aria-disabled={formDisabled ? 'true' : undefined} className={`space-y-8 ${formDisabled ? 'opacity-50 pointer-events-none' : ''}`}>
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

                  <p className="text-xs text-gray-400 text-center">
                    Your answers are encrypted and can only be read by the form creator.
                    Your NEAR account ID and submission timestamp will be publicly recorded on the NEAR blockchain.
                  </p>

                  <button
                    type="submit"
                    disabled={submitting || formDisabled}
                    className="w-full bg-brand-600 text-white py-3 rounded-md font-medium hover:bg-brand-700 disabled:bg-gray-400 disabled:cursor-not-allowed transition"
                  >
                    {submitting ? 'Submitting...' : 'Submit Form'}
                  </button>
                </form>
              </>
            )}
          </div>
        </div>
        </div>
      </div>
    </>
  );
}
