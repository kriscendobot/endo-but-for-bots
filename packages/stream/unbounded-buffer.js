// @ts-check

import harden from '@endo/harden';
import { makePromiseKit } from '@endo/promise-kit';

import { makeQueue } from './index.js';

/** @import { AsyncQueue } from './types.js' */
/** @import { BufferKit } from './buffer.js' */

const { freeze } = Object;

/**
 * Make an unbounded asynchronous buffer over a promise queue.
 *
 * @template TValue
 * @template [TReturn=undefined]
 * @returns {BufferKit<TValue, TReturn>}
 */
export const makeUnboundedBuffer = () => {
  /** @type {AsyncQueue<IteratorResult<TValue, TReturn>>} */
  const queue = makeQueue();
  const {
    promise: rawSinkFinished,
    resolve: resolveSinkFinished,
    reject: rejectSinkFinished,
  } = makePromiseKit();
  /** @type {Promise<IteratorResult<TValue, TReturn>>} */
  const sinkFinished = rawSinkFinished;
  // Terminal rejection is observed by a pending or subsequent sink.next().
  // Registering this inert handler avoids reporting it before a consumer pulls.
  void sinkFinished.catch(() => undefined);

  let springFinished = false;
  let sinkFinishedEarly = false;

  /** @param {IteratorResult<TValue, TReturn>} result */
  const finishSink = result => {
    sinkFinishedEarly = true;
    springFinished = true;
    resolveSinkFinished(result);
  };

  const spring = harden({
    /** @param {TValue | PromiseLike<TValue>} value */
    next(value) {
      if (springFinished) return;
      const result = Promise.resolve(value).then(
        resolvedValue =>
          /** @type {IteratorYieldResult<TValue>} */ (
            freeze({ value: resolvedValue, done: false })
          ),
      );
      // As with makeQueue(), the reader is responsible for observing a rejected
      // value. Keep a fire-and-forget rejected input from becoming unhandled
      // before the reader gets there.
      void result.catch(() => undefined);
      queue.put(result);
    },
    /** @param {TReturn} value */
    return(value) {
      if (springFinished) return;
      springFinished = true;
      queue.put(freeze({ value, done: true }));
    },
    /** @param {Error} error */
    throw(error) {
      if (springFinished) return;
      springFinished = true;
      const rejection = Promise.reject(error);
      void rejection.catch(() => undefined);
      queue.put(rejection);
      queue.put(
        /** @type {IteratorReturnResult<TReturn>} */ (
          freeze({ value: undefined, done: true })
        ),
      );
    },
  });

  const sink = harden({
    next() {
      if (sinkFinishedEarly) return sinkFinished;
      return Promise.race([queue.get(), sinkFinished]).then(result => {
        if (result.done) {
          sinkFinishedEarly = true;
          resolveSinkFinished(result);
        }
        return result;
      });
    },
    /** @param {TReturn} value */
    return(value) {
      if (!sinkFinishedEarly) {
        finishSink(freeze({ value, done: true }));
      }
      return sinkFinished;
    },
    /** @param {Error} error */
    throw(error) {
      if (!sinkFinishedEarly) {
        sinkFinishedEarly = true;
        springFinished = true;
        rejectSinkFinished(error);
      }
      return sinkFinished;
    },
    [Symbol.asyncIterator]() {
      return sink;
    },
  });

  return harden({ spring, sink });
};
harden(makeUnboundedBuffer);
