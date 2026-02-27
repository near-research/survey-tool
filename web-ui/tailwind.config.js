/** @type {import('tailwindcss').Config} */
module.exports = {
  content: [
    './src/pages/**/*.{js,ts,jsx,tsx,mdx}',
    './src/components/**/*.{js,ts,jsx,tsx,mdx}',
    './src/app/**/*.{js,ts,jsx,tsx,mdx}',
  ],
  theme: {
    extend: {
      colors: {
        brand: {
          50: '#eefffe',
          100: '#c5fffd',
          200: '#8bfffb',
          300: '#49f5ef',
          400: '#17d9d4',
          500: '#0bbdb9',
          600: '#099997',
          700: '#0d7a79',
          800: '#106061',
          900: '#124f50',
        },
      },
    },
  },
  plugins: [],
}
