// @ts-check

import harden from '@endo/harden';

import { makeUnboundedBuffer } from './unbounded-buffer.js';

/** @import { BufferKit } from './buffer.js' */

/**
 * Make an unbounded asynchronous buffer.
 *
 * @template TValue
 * @template [TReturn=undefined]
 * @returns {BufferKit<TValue, TReturn>}
 */
export const makeBuffer = () => makeUnboundedBuffer();
harden(makeBuffer);
