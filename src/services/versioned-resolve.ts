import type { ServiceResult } from '../types/errors.js';

interface VersionedResolveOptions<TValue, TSnapshot> {
  resolve: () => Promise<ServiceResult<TValue>>;
  isCurrent: () => boolean;
  snapshot: () => TSnapshot;
  start: () => void;
  succeed: (value: TValue) => void | Promise<void>;
  restore: (snapshot: TSnapshot) => void;
  fail: (message: string) => void | Promise<void>;
}

export async function runVersionedResolve<TValue, TSnapshot>(
  options: VersionedResolveOptions<TValue, TSnapshot>,
): Promise<boolean> {
  const snapshot = options.snapshot();
  options.start();

  try {
    const result = await options.resolve();
    if (!options.isCurrent()) return false;
    if (!result.ok) throw new Error(result.error.message);
    await options.succeed(result.value);
    return true;
  } catch (error: unknown) {
    if (!options.isCurrent()) return false;
    options.restore(snapshot);
    await options.fail(error instanceof Error ? error.message : '解析失败');
    return false;
  }
}
