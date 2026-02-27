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
        <link rel="icon" href="/favicon.ico" sizes="any" />
        <link rel="icon" type="image/png" sizes="32x32" href="/favicon-32x32.png" />
        <link rel="icon" type="image/png" sizes="16x16" href="/favicon-16x16.png" />
        <link rel="apple-touch-icon" sizes="180x180" href="/apple-touch-icon.png" />
        <link rel="manifest" href="/site.webmanifest" />
        <meta name="theme-color" content="#2563eb" />
      </Head>
      {walletError && (
        <div style={{ background: '#fef2f2', color: '#991b1b', padding: '12px 16px', textAlign: 'center', fontSize: '14px' }}>
          Wallet initialization failed: {walletError}
        </div>
      )}
      <Component {...pageProps} />
    </>
  );
}
