/**
 * Client-side EC01 encryption for form answers.
 *
 * Produces byte-identical output to the Rust encrypt_for_form() in
 * wasi-near-forms-ark/src/crypto.rs so the WASI module can decrypt
 * using the same key derivation and EC01 format.
 *
 * Format: EC01 (4) || ephemeral_pubkey (33) || nonce (12) || ciphertext+tag
 */

import { secp256k1 } from '@noble/curves/secp256k1.js';
import { sha256 } from '@noble/hashes/sha2.js';
import { hkdf } from '@noble/hashes/hkdf.js';
import { chacha20poly1305 } from '@noble/ciphers/chacha.js';
import { randomBytes, bytesToHex, concatBytes } from '@noble/hashes/utils.js';

// Must match Rust constants exactly
const EC01_MAGIC = new Uint8Array([0x45, 0x43, 0x30, 0x31]); // "EC01"
const DERIVATION_PREFIX = 'near-forms:v1:';
const HKDF_INFO = 'near-forms:v1:ecdh';

// secp256k1 curve order (well-known constant)
const CURVE_ORDER = 0xFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFEBAAEDCE6AF48A03BBFD25E8CD0364141n;

const encoder = new TextEncoder();

// noble/curves v2 API: Point class with fromHex, BASE, multiply, add, toBytes
const Point = secp256k1.Point;

/**
 * Derive the form-specific public key from the master public key.
 *
 * Matches Rust: form_pubkey = master_pubkey + SHA256("near-forms:v1:" + form_id) * G
 */
function deriveFormPublicKey(masterPubKeyHex: string, formId: string): Uint8Array {
  // Compute tweak: SHA256(prefix + form_id)
  const tweakInput = concatBytes(
    encoder.encode(DERIVATION_PREFIX),
    encoder.encode(formId),
  );
  const tweakHash = sha256(tweakInput);

  // Reduce tweak modulo curve order (matches Rust SecretKey::parse_slice behavior)
  const tweakScalar = bytesToBigInt(tweakHash) % CURVE_ORDER;
  if (tweakScalar === 0n) {
    throw new Error('Tweak is zero (astronomically unlikely)');
  }

  // tweak * G
  const tweakPoint = Point.BASE.multiply(tweakScalar);

  // master_pubkey + tweak * G
  const masterPoint = Point.fromHex(masterPubKeyHex);
  const formPoint = masterPoint.add(tweakPoint);

  return formPoint.toBytes(true); // compressed, 33 bytes
}

/**
 * Encrypt plaintext using EC01 format (ECDH + ChaCha20-Poly1305).
 *
 * Returns hex-encoded EC01 blob.
 */
function encryptEC01(formPubKey: Uint8Array, plaintext: Uint8Array): string {
  // 1. Generate ephemeral keypair
  const ephemeralPriv = secp256k1.utils.randomSecretKey();
  const ephemeralPub = secp256k1.getPublicKey(ephemeralPriv, true); // compressed, 33 bytes

  // 2. ECDH: shared_point = form_pubkey * ephemeral_privkey
  const sharedPoint = secp256k1.getSharedSecret(ephemeralPriv, formPubKey, true); // compressed
  const sharedX = sharedPoint.slice(1); // x-coordinate only (skip prefix byte), 32 bytes

  // 3. HKDF-SHA256: derive 32-byte key
  // salt=undefined matches Rust Hkdf::new(None, shared_x) which uses zero-filled salt
  const key = hkdf(sha256, sharedX, undefined, encoder.encode(HKDF_INFO), 32);

  // 4. ChaCha20-Poly1305 encrypt
  const nonce = randomBytes(12);
  const cipher = chacha20poly1305(key, nonce);
  const ciphertext = cipher.encrypt(plaintext); // includes 16-byte Poly1305 tag

  // 5. Assemble: EC01 || ephemeral_pubkey || nonce || ciphertext+tag
  const output = concatBytes(EC01_MAGIC, ephemeralPub, nonce, ciphertext);

  return bytesToHex(output);
}

/**
 * Encrypt form answers for submission.
 *
 * @param masterPubKeyHex - Hex-encoded compressed master public key (66 chars)
 * @param formId - Form UUID
 * @param answers - Answer map {question_id: answer_value}
 * @returns Hex-encoded EC01 blob
 */
export function encryptFormAnswers(
  masterPubKeyHex: string,
  formId: string,
  answers: Record<string, unknown>,
): string {
  const formPubKey = deriveFormPublicKey(masterPubKeyHex, formId);
  const plaintext = encoder.encode(JSON.stringify(answers));
  return encryptEC01(formPubKey, plaintext);
}

/** Convert Uint8Array to BigInt (big-endian) */
function bytesToBigInt(bytes: Uint8Array): bigint {
  let result = 0n;
  for (const byte of bytes) {
    result = (result << 8n) | BigInt(byte);
  }
  return result;
}
