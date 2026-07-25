/** A value or a promise for a value. */
export type MaybePromise<TValue> = TValue | PromiseLike<TValue>;

/** The fire-and-forget producer half of a buffer. */
export interface BufferSpring<TValue, TReturn = undefined> {
  next(value: MaybePromise<TValue>): void;
  return(value: TReturn): void;
  throw(error: Error): void;
}

/** The pullable consumer half of a buffer. */
export interface BufferSink<TValue, TReturn = undefined> {
  next(): Promise<IteratorResult<TValue, TReturn>>;
  return(value: TReturn): Promise<IteratorResult<TValue, TReturn>>;
  throw(error: Error): Promise<IteratorResult<TValue, TReturn>>;
  [Symbol.asyncIterator](): BufferSink<TValue, TReturn>;
}

/** The producer and consumer halves of a buffer. */
export interface BufferKit<TValue, TReturn = undefined> {
  spring: BufferSpring<TValue, TReturn>;
  sink: BufferSink<TValue, TReturn>;
}

/** Make an unbounded asynchronous buffer. */
export declare function makeBuffer<TValue, TReturn = undefined>(): BufferKit<
  TValue,
  TReturn
>;
