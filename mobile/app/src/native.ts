import { requireNativeModule } from 'expo-modules-core';
import { Snapshot, isSnapshot } from './types';

type Envelope = { ok: true; data: unknown } | { ok: false; error: { code?: string; message: string } };
type NativeBridge = { request(requestJSON: string): Promise<string> };

let bridge: NativeBridge | null | undefined;
function getBridge(): NativeBridge | null {
  if (bridge !== undefined) return bridge;
  try {
    bridge = requireNativeModule<NativeBridge>('MyProxy');
  } catch {
    bridge = null;
  }
  return bridge;
}

export class BridgeError extends Error {
  constructor(message: string, readonly code = 'native_unavailable') { super(message); }
}

export async function request(operation: Record<string, unknown>): Promise<Snapshot> {
  const native = getBridge();
  if (!native) throw new BridgeError('原生运行模块尚未接入。请使用带 VPN 模块的安装包。');
  let raw: string;
  try { raw = await native.request(JSON.stringify(operation)); } catch { throw new BridgeError('无法联系代理运行模块，请稍后重试。', 'bridge_error'); }
  let envelope: Envelope;
  try { envelope = JSON.parse(raw) as Envelope; } catch { throw new BridgeError('运行模块返回了无法识别的数据。', 'invalid_response'); }
  if (!envelope.ok) throw new BridgeError(envelope.error.message, envelope.error.code);
  if (!isSnapshot(envelope.data)) throw new BridgeError('运行模块没有返回完整的当前状态。', 'invalid_response');
  return envelope.data;
}

export async function send(operation: Record<string, unknown>, current: Snapshot): Promise<Snapshot> {
  // Every mutation returns the authoritative snapshot; no optimistic connection state is kept in JS.
  void current;
  return request(operation);
}
