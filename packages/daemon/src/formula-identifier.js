// @ts-check
/// <ref types="ses">

/** @import { IdRecord, ParseIdRecord, FormulaNumber, NodeNumber, FormulaIdentifier } from './types.js' */

import { makeError, q } from '@endo/errors';

// Terminology (see designs/daemon-locator-terminology.md): a formula key
// (`FormulaKey`, the `id` throughout this module) is
// `{formulaAddress}:{peerKey}`.  The formula address (`FormulaAddress`) is a
// content address (SHA-256) or capability address (random 256-bit); the peer
// key (`PeerKey`) is the owning peer's Ed25519 public key.  The legacy record
// field names `number` and `node` hold the formula address and peer key
// respectively; they are kept for signature compatibility.

const numberPattern = /^[0-9a-f]{64}$/;
const idPattern = /^(?<number>[0-9a-f]{64}):(?<node>[0-9a-f]{64})$/;

/**
 * @param {string} allegedNumber - The formula address or peer key to test.
 */
export const isValidNumber = allegedNumber =>
  typeof allegedNumber === 'string' && numberPattern.test(allegedNumber);

/**
 * @param {string} allegedNumber - The formula address or peer key to test.
 */
export const assertValidNumber = allegedNumber => {
  if (!isValidNumber(allegedNumber)) {
    throw makeError(`Invalid number ${q(allegedNumber)}`);
  }
};

/**
 * Assert that a string is a valid formula address.
 *
 * @param {string} allegedFormulaNumber
 * @returns {asserts allegedFormulaNumber is FormulaNumber}
 */
export const assertFormulaNumber = allegedFormulaNumber => {
  if (!isValidNumber(allegedFormulaNumber)) {
    throw makeError(`Invalid formula address ${q(allegedFormulaNumber)}`);
  }
};

/**
 * Assert that a string is a valid peer key.
 *
 * @param {string} allegedNodeNumber
 * @returns {asserts allegedNodeNumber is NodeNumber}
 */
export const assertNodeNumber = allegedNodeNumber => {
  if (!isValidNumber(allegedNodeNumber)) {
    throw makeError(`Invalid peer key ${q(allegedNodeNumber)}`);
  }
};

/**
 * Assert that a string is a valid formula key.
 *
 * @param {string} id
 * @param {string} [petName]
 * @returns {asserts id is FormulaIdentifier}
 */
export const assertValidId = (id, petName) => {
  if (typeof id !== 'string' || !idPattern.test(id)) {
    let message = `Invalid formula key ${q(id)}`;
    if (petName !== undefined) {
      message += ` for pet name ${q(petName)}`;
    }
    throw new Error(message);
  }
};

/**
 * Parse a formula key into its formula address (`number`) and peer key
 * (`node`).
 *
 * @param {string} id
 * @returns {ParseIdRecord}
 */
export const parseId = id => {
  const match = idPattern.exec(id);
  if (match === null) {
    throw makeError(`Invalid formula key ${q(id)}`);
  }
  const { groups } = match;
  if (groups === undefined) {
    throw makeError(
      `Programmer invariant failure: expected match groups, formula identifier was ${q(
        id,
      )}`,
    );
  }

  const { number, node } = groups;
  const formulaNumber = /** @type {FormulaNumber} */ (number);
  const nodeNumber = /** @type {NodeNumber} */ (node);
  const formulaId = /** @type {FormulaIdentifier} */ (id);
  return {
    number: formulaNumber,
    node: nodeNumber,
    id: formulaId,
  };
};

/**
 * Format a formula address (`number`) and peer key (`node`) as a formula
 * key.
 *
 * @param {IdRecord} formulaRecord
 * @returns {FormulaIdentifier}
 */
export const formatId = ({ number, node }) => {
  const id = `${String(number)}:${String(node)}`;
  assertValidId(id);
  return /** @type {FormulaIdentifier} */ (id);
};
