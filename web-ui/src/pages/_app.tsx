import '@/styles/globals.css';
import '@near-wallet-selector/modal-ui/styles.css';
import type { AppProps } from 'next/app';
import Head from 'next/head';
import { useEffect, useState } from 'react';
import { initWalletSelector } from '@/lib/near';

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
    <>
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
    </>
  );
}
