/**
 * Encryption round-trip tests for the EC01 format.
 *
 * Verifies that encryptEC01 and decryptEC01 are consistent, and that the key
 * derivation (deriveFormPublicKey) produces deterministic results matching the
 * Rust golden test vector.
 */

import { describe, it, expect } from 'vitest';
import { secp256k1 } from '@noble/curves/secp256k1.js';
import { bytesToHex, hexToBytes, concatBytes } from '@noble/hashes/utils.js';
import { encryptFormAnswers, encryptEC01, generateSessionKeypair, decryptEC01, deriveFormPublicKey } from './crypto';

// ==================== Constants (must match crypto.ts and Rust) ====================

const EC01_MAGIC = new Uint8Array([0x45, 0x43, 0x30, 0x31]); // "EC01"

const encoder = new TextEncoder();
const Point = secp256k1.Point;

// Known test key (private key = 1, simplest possible)
const TEST_MASTER_KEY = '0000000000000000000000000000000000000000000000000000000000000001';
const TEST_FORM_ID = 'daf14a0c-20f7-4199-a07b-c6456d53ef2d';

// The compressed public key for private key = 1 (the generator point G)
const TEST_MASTER_PUBKEY = '0279be667ef9dcbbac55a06295ce870b07029bfcdb2dce28d959f2815b16f81798';

// ==================== Tests ====================

describe('EC01 encrypt/decrypt round-trip', () => {
  it('encrypts and decrypts simple plaintext', () => {
    const { privateKey, publicKeyHex } = generateSessionKeypair();
    const pubKeyBytes = hexToBytes(publicKeyHex);
    const plaintext = encoder.encode('hello near-forms');

    const encryptedHex = encryptEC01(pubKeyBytes, plaintext);
    const decrypted = decryptEC01(privateKey, encryptedHex);

    expect(new TextDecoder().decode(decrypted)).toBe('hello near-forms');
  });

  it('encrypts and decrypts empty plaintext', () => {
    const { privateKey, publicKeyHex } = generateSessionKeypair();
    const pubKeyBytes = hexToBytes(publicKeyHex);

    const encryptedHex = encryptEC01(pubKeyBytes, new Uint8Array(0));
    const decrypted = decryptEC01(privateKey, encryptedHex);

    expect(decrypted.length).toBe(0);
  });

  it('encrypts and decrypts JSON answers', () => {
    const { privateKey, publicKeyHex } = generateSessionKeypair();
    const pubKeyBytes = hexToBytes(publicKeyHex);
    const answers = { q1: 'Option A', q2: ['Choice 1', 'Choice 2'], q3: 'Open text' };
    const plaintext = encoder.encode(JSON.stringify(answers));

    const encryptedHex = encryptEC01(pubKeyBytes, plaintext);
    const decrypted = decryptEC01(privateKey, encryptedHex);
    const parsed = JSON.parse(new TextDecoder().decode(decrypted));

    expect(parsed).toEqual(answers);
  });

  it('produces unique ciphertext for same plaintext (ephemeral keys)', () => {
    const { privateKey, publicKeyHex } = generateSessionKeypair();
    const pubKeyBytes = hexToBytes(publicKeyHex);
    const plaintext = encoder.encode('same message');

    const enc1 = encryptEC01(pubKeyBytes, plaintext);
    const enc2 = encryptEC01(pubKeyBytes, plaintext);

    expect(enc1).not.toBe(enc2);

    // Both decrypt to the same plaintext
    expect(new TextDecoder().decode(decryptEC01(privateKey, enc1))).toBe('same message');
    expect(new TextDecoder().decode(decryptEC01(privateKey, enc2))).toBe('same message');
  });

  it('fails to decrypt with wrong private key', () => {
    const { publicKeyHex } = generateSessionKeypair();
    const wrongKeypair = generateSessionKeypair();
    const pubKeyBytes = hexToBytes(publicKeyHex);

    const encryptedHex = encryptEC01(pubKeyBytes, encoder.encode('secret'));

    expect(() => decryptEC01(wrongKeypair.privateKey, encryptedHex)).toThrow();
  });

  it('fails on tampered ciphertext', () => {
    const { privateKey, publicKeyHex } = generateSessionKeypair();
    const pubKeyBytes = hexToBytes(publicKeyHex);

    const encryptedHex = encryptEC01(pubKeyBytes, encoder.encode('secret data'));
    const bytes = hexToBytes(encryptedHex);
    // Flip a byte in the ciphertext (after header + pubkey + nonce)
    bytes[4 + 33 + 12 + 1] ^= 0xFF;
    const tamperedHex = bytesToHex(bytes);

    expect(() => decryptEC01(privateKey, tamperedHex)).toThrow();
  });

  it('rejects invalid magic bytes', () => {
    const { privateKey } = generateSessionKeypair();
    const badHex = bytesToHex(new Uint8Array(100)); // all zeros

    expect(() => decryptEC01(privateKey, badHex)).toThrow('Invalid EC01 magic bytes');
  });

  it('rejects too-short blob', () => {
    const { privateKey } = generateSessionKeypair();
    const shortHex = bytesToHex(concatBytes(EC01_MAGIC, new Uint8Array(10)));

    expect(() => decryptEC01(privateKey, shortHex)).toThrow('too short');
  });
});

describe('encryptFormAnswers (full flow)', () => {
  it('encrypts form answers with derived form key', () => {
    // This tests the full client-side flow: derive form key, encrypt answers
    const answers = { q1: 'Yes', q2: ['A', 'B'] };
    const encryptedHex = encryptFormAnswers(TEST_MASTER_PUBKEY, TEST_FORM_ID, answers);

    // Verify EC01 format
    expect(encryptedHex.substring(0, 8)).toBe('45433031'); // "EC01" in hex

    // We can't decrypt here (need form private key from Rust side),
    // but we verify the blob is well-formed
    const bytes = hexToBytes(encryptedHex);
    expect(bytes.length).toBeGreaterThan(4 + 33 + 12 + 16);
  });
});

describe('Key derivation', () => {
  it('deriveFormPublicKey is deterministic', () => {
    const pk1 = deriveFormPublicKey(TEST_MASTER_PUBKEY, TEST_FORM_ID);
    const pk2 = deriveFormPublicKey(TEST_MASTER_PUBKEY, TEST_FORM_ID);

    expect(bytesToHex(pk1)).toBe(bytesToHex(pk2));
  });

  it('different form IDs produce different keys', () => {
    const pk1 = deriveFormPublicKey(TEST_MASTER_PUBKEY, 'form-a');
    const pk2 = deriveFormPublicKey(TEST_MASTER_PUBKEY, 'form-b');

    expect(bytesToHex(pk1)).not.toBe(bytesToHex(pk2));
  });

  it('master pubkey for private key 1 matches known value', () => {
    // Private key = 1 => public key = generator point G
    const pubkey = secp256k1.getPublicKey(hexToBytes(TEST_MASTER_KEY), true);
    expect(bytesToHex(pubkey)).toBe(TEST_MASTER_PUBKEY);
  });

  it('derived form pubkey is a valid secp256k1 point', () => {
    const formPubKey = deriveFormPublicKey(TEST_MASTER_PUBKEY, TEST_FORM_ID);
    expect(formPubKey.length).toBe(33);
    expect(formPubKey[0] === 0x02 || formPubKey[0] === 0x03).toBe(true);

    // Verify it's a valid point by parsing it
    const point = Point.fromHex(bytesToHex(formPubKey));
    expect(point.toBytes(true).length).toBe(33);
  });

  it('golden vector: form pubkey matches Rust derivation', () => {
    // This value must match the Rust golden_derive_form_pubkey test output.
    // If this test fails after a code change, both TypeScript and Rust must be updated together.
    const formPubKey = deriveFormPublicKey(TEST_MASTER_PUBKEY, TEST_FORM_ID);
    const formPubKeyHex = bytesToHex(formPubKey);

    // Pin form pubkey — must match Rust golden_derive_form_pubkey test
    expect(formPubKeyHex).toBe('02257731f1d53b68b0c8e8602250746131b1b037556343b4f666c9ac753e5cc4ea');
  });
});

describe('Full submit→read→decrypt integration flow', () => {
  it('simulates the complete encrypted form lifecycle', () => {
    // Step 1: Client encrypts answers with derived form public key (SubmitForm)
    const answers = { q1: 'Option A', q2: ['Choice 1', 'Choice 2'], q3: 'Open text response' };
    const encryptedSubmission = encryptFormAnswers(TEST_MASTER_PUBKEY, TEST_FORM_ID, answers);

    // Verify EC01 format
    expect(encryptedSubmission.substring(0, 8)).toBe('45433031');

    // Step 2: Simulate WASI re-encrypting decrypted payload to caller's session key (ReadResponses)
    // (WASI decryption is tested in Rust; here we test the session key flow)
    const session = generateSessionKeypair();
    const responsePayload = JSON.stringify({
      responses: [{
        submitter_id: 'alice.testnet',
        answers,
        submitted_at: '2026-03-03T00:00:00Z',
      }],
      total_count: 1,
      has_more: false,
      skipped_count: 0,
    });

    const encryptedResponse = encryptEC01(
      hexToBytes(session.publicKeyHex),
      encoder.encode(responsePayload),
    );

    // Step 3: Client decrypts response with session private key
    const decrypted = decryptEC01(session.privateKey, encryptedResponse);
    const parsed = JSON.parse(new TextDecoder().decode(decrypted));

    // Verify full round-trip integrity
    expect(parsed.responses[0].submitter_id).toBe('alice.testnet');
    expect(parsed.responses[0].answers).toEqual(answers);
    expect(parsed.total_count).toBe(1);
    expect(parsed.has_more).toBe(false);

    // Step 4: Zero the session key (matches browser cleanup behavior)
    session.privateKey.fill(0);
    expect(session.privateKey.every(b => b === 0)).toBe(true);
  });

  it('handles multiple submissions in a single ReadResponses page', () => {
    const session = generateSessionKeypair();

    // Simulate a page with 3 responses
    const responses = Array.from({ length: 3 }, (_, i) => ({
      submitter_id: `user${i}.testnet`,
      answers: { q1: `Answer ${i}` },
      submitted_at: `2026-03-0${i + 1}T00:00:00Z`,
    }));

    const payload = JSON.stringify({
      responses,
      total_count: 10,
      has_more: true,
      skipped_count: 1,
      next_offset: 4,
    });

    const encrypted = encryptEC01(hexToBytes(session.publicKeyHex), encoder.encode(payload));
    const decrypted = JSON.parse(new TextDecoder().decode(decryptEC01(session.privateKey, encrypted)));

    expect(decrypted.responses).toHaveLength(3);
    expect(decrypted.total_count).toBe(10);
    expect(decrypted.has_more).toBe(true);
    expect(decrypted.skipped_count).toBe(1);
    expect(decrypted.next_offset).toBe(4);
    expect(decrypted.responses[2].submitter_id).toBe('user2.testnet');
  });
});

describe('generateSessionKeypair', () => {
  it('produces valid keypairs', () => {
    const { privateKey, publicKeyHex } = generateSessionKeypair();

    expect(privateKey.length).toBe(32);
    expect(publicKeyHex.length).toBe(66); // compressed pubkey = 33 bytes = 66 hex
    expect(publicKeyHex.startsWith('02') || publicKeyHex.startsWith('03')).toBe(true);
  });

  it('produces unique keypairs', () => {
    const kp1 = generateSessionKeypair();
    const kp2 = generateSessionKeypair();

    expect(kp1.publicKeyHex).not.toBe(kp2.publicKeyHex);
  });
});
