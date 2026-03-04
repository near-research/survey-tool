import '@/styles/globals.css';
import '@near-wallet-selector/modal-ui/styles.css';
import type { AppProps } from 'next/app';
import Head from 'next/head';
import { Component, useEffect, useState } from 'react';
import type { ErrorInfo, ReactNode } from 'react';
import { initWalletSelector } from '@/lib/near';

interface ErrorBoundaryProps {
  children: ReactNode;
}

interface ErrorBoundaryState {
  hasError: boolean;
  error: Error | null;
}

class ErrorBoundary extends Component<ErrorBoundaryProps, ErrorBoundaryState> {
  constructor(props: ErrorBoundaryProps) {
    super(props);
    this.state = { hasError: false, error: null };
  }

  static getDerivedStateFromError(error: Error): ErrorBoundaryState {
    return { hasError: true, error };
  }

  componentDidCatch(error: Error, info: ErrorInfo) {
    console.error('Uncaught error:', error, info.componentStack);
  }

  render() {
    if (this.state.hasError) {
      return (
        <div className="min-h-screen bg-gradient-to-br from-blue-50 to-brand-100 flex items-center justify-center px-4">
          <div className="bg-white rounded-lg shadow-lg p-8 max-w-md text-center">
            <h2 className="text-2xl font-bold mb-4 text-gray-800">Something went wrong</h2>
            <p className="text-gray-600 mb-6">
              An unexpected error occurred. Please try refreshing the page.
            </p>
            {this.state.error && (
              <p className="text-sm text-gray-400 mb-6 font-mono break-all">
                {this.state.error.message}
              </p>
            )}
            <button
              onClick={() => window.location.reload()}
              className="bg-brand-600 text-white px-6 py-2 rounded-md font-medium hover:bg-brand-700 transition"
            >
              Refresh Page
            </button>
          </div>
        </div>
      );
    }

    return this.props.children;
  }
}

export default function App({ Component, pageProps }: AppProps) {
  const [walletError, setWalletError] = useState<string | null>(null);

  useEffect(() => {
    async function init() {
      try {
        await initWalletSelector();
      } catch (error) {
        console.error('Failed to initialize wallet:', error);
        setWalletError(error instanceof Error ? error.message : String(error));
      }
    }

    init();
  }, []);

  return (
    <ErrorBoundary>
      <Head>
        <link rel="icon" href="data:image/svg+xml,<svg xmlns='http://www.w3.org/2000/svg' viewBox='0 0 100 100'><text y='.9em' font-size='90'>📋</text></svg>" />
        <link rel="manifest" href="/site.webmanifest" />
        <meta name="theme-color" content="#17d9d4" />
      </Head>
      {walletError && (
        <div className="bg-red-50 text-red-800 px-4 py-3 text-center text-sm">
          Wallet initialization failed: {walletError}
        </div>
      )}
      <Component {...pageProps} />
    </ErrorBoundary>
  );
}
