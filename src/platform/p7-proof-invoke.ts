/// <reference types="vite/client" />
import { invoke } from '@tauri-apps/api/core';

export function invokeMedia<T>(command: string, args: Record<string, unknown>): Promise<T> {
  if (import.meta.env?.VITE_P7_OVERLAP_PROOF === '1') {
    const proof = (window as typeof window & {
      __p7ProofInvoke?: (command: string, args: Record<string, unknown>, native: () => Promise<T>) => Promise<T>;
    }).__p7ProofInvoke;
    if (proof) return proof(command, args, () => invoke<T>(command, args));
  }
  return invoke<T>(command, args);
}
