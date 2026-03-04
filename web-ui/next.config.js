const isDev = process.env.NODE_ENV === 'development';

/** @type {import('next').NextConfig} */
const nextConfig = {
  reactStrictMode: true,
  output: 'standalone',
  webpack: (config, { isServer }) => {
    if (!isServer) {
      // Polyfills for near-api-js
      config.resolve.fallback = {
        ...config.resolve.fallback,
        fs: false,
        net: false,
        tls: false,
        crypto: false,
      };
    }
    return config;
  },
  async headers() {
    return [
      {
        source: '/:path*',
        headers: [
          {
            key: 'X-Frame-Options',
            value: 'DENY',
          },
          {
            key: 'X-Content-Type-Options',
            value: 'nosniff',
          },
          {
            key: 'Referrer-Policy',
            value: 'strict-origin-when-cross-origin',
          },
          {
            key: 'Strict-Transport-Security',
            value: 'max-age=31536000; includeSubDomains',
          },
          {
            key: 'Content-Security-Policy',
            value: [
              "default-src 'self'",
              "script-src 'self' 'unsafe-inline' 'unsafe-eval'",
              "style-src 'self' 'unsafe-inline' https://fonts.googleapis.com",
              "img-src 'self' data: blob: https:",
              "font-src 'self' https://fonts.gstatic.com",
              [
                "connect-src 'self'",
                'https://rpc.mainnet.near.org',
                'https://rpc.testnet.near.org',
                'https://api.kitwallet.app',
                'https://api.testnet.kitwallet.app',
                'https://*.near.org',
                'https://*.mynearwallet.com',
                'https://*.meteorwallet.app',
                'https://*.herewallet.app',
                'https://*.hot-labs.org',
                'https://*.nearmobile.app',
                'https://intear.app',
                'https://*.up.railway.app',
                'https://*.fastnear.com',
                ...(isDev ? ['http://localhost:*', 'ws://localhost:*'] : []),
              ].join(' '),
              [
                "frame-src 'self'",
                'https://*.mynearwallet.com',
                'https://*.meteorwallet.app',
                'https://*.herewallet.app',
                'https://*.nearmobile.app',
                'https://intear.app',
              ].join(' '),
              "frame-ancestors 'none'",
              "base-uri 'self'",
              "form-action 'self'",
            ].join('; '),
          },
        ],
      },
    ];
  },
}

module.exports = nextConfig
