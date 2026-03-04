import Head from 'next/head';
import Link from 'next/link';
import type { FormData } from '@/lib/types';
import { useFetchWithTimeout, getFormApiUrl } from '@/lib/hooks';

export default function HomePage() {
  const formUrl = getFormApiUrl();
  const { data: form, loading, error } = useFetchWithTimeout<FormData>(formUrl);

  if (loading) {
    return (
      <div className="min-h-screen bg-gradient-to-br from-blue-50 to-brand-100 flex items-center justify-center">
        <p className="text-gray-500 animate-pulse">Loading...</p>
      </div>
    );
  }

  if (error || !form) {
    return (
      <div className="min-h-screen bg-gradient-to-br from-blue-50 to-brand-100 flex items-center justify-center">
        <p className="text-red-600">{error || 'Failed to load form'}</p>
      </div>
    );
  }

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

        <div className="max-w-2xl mx-auto py-12 px-4">
          <div className="bg-white rounded-lg shadow-lg p-8">
            <h2 className="text-3xl font-bold text-gray-900 mb-2">{form.title}</h2>
            <p className="text-gray-600 mb-8">
              Fill out and sign this form with your NEAR wallet. Your responses will be encrypted and stored.
            </p>

            <div className="bg-blue-50 border border-blue-200 rounded-lg p-4 mb-8">
              <p className="text-sm text-blue-900">
                <strong>How it works:</strong> Your answers will be encrypted with the form creator's public key. Only they can decrypt and read your responses.
              </p>
            </div>

            <Link
              href={`/forms/${form.id}`}
              className="inline-block bg-brand-600 text-white px-6 py-3 rounded-lg font-medium hover:bg-brand-700"
            >
              Fill Out Form
            </Link>
          </div>
        </div>
      </div>
    </>
  );
}
