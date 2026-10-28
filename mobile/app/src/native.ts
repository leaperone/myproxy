import { requireNativeModule } from 'expo-modules-core';
import { Connection, Group, GroupKind, Member, Node, Rule, Runtime, Snapshot, Source } from './types';

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

function record(value: unknown): Record<string, unknown> | null {
  return typeof value === 'object' && value !== null && !Array.isArray(value) ? Object.fromEntries(Object.entries(value)) : null;
}
function string(value: unknown): string | null { return typeof value === 'string' ? value : null; }
function nullableString(value: unknown): string | null | undefined { return value === null ? null : typeof value === 'string' ? value : undefined; }
function number(value: unknown): number | null { return typeof value === 'number' && Number.isFinite(value) ? value : null; }
function array(value: unknown): unknown[] | null { return Array.isArray(value) ? value : null; }

function parseSnapshot(value: unknown): Snapshot | null {
  const root = record(value);
  if (!root) return null;
  const revision = number(root.revision);
  const mode = root.mode === 'global' || root.mode === 'rule' ? root.mode : null;
  const autoConnect = typeof root.autoConnect === 'boolean' ? root.autoConnect : null;
  const selected = string(root.selected);
  const fallback = string(root.fallback);
  const capabilities = record(root.capabilities);
  const dynamicDirect = capabilities ? capabilities.dynamicDirect : undefined;
  const appRouting = capabilities ? capabilities.appRouting : undefined;
  if (revision === null || mode === null || autoConnect === null || selected === null || fallback === null ||
      typeof dynamicDirect !== 'boolean' || typeof appRouting !== 'boolean') return null;

  const sources = parseSources(root.sources);
  const nodes = parseNodes(root.nodes);
  const groups = parseGroups(root.groups);
  const rules = parseRules(root.rules);
  const warnings = parseStrings(root.warnings);
  const runtime = parseRuntime(root.runtime);
  if (!sources || !nodes || !groups || !rules || !warnings || !runtime) return null;
  return { revision, mode, autoConnect, selected, fallback, capabilities: { dynamicDirect, appRouting }, sources, nodes, groups, rules, warnings, runtime };
}

function parseStrings(value: unknown): string[] | null {
  const values = array(value);
  if (!values) return null;
  const result: string[] = [];
  for (const item of values) { const parsed = string(item); if (parsed === null) return null; result.push(parsed); }
  return result;
}
function parseSources(value: unknown): Source[] | null {
  const values = array(value); if (!values) return null; const result: Source[] = [];
  for (const item of values) { const source = record(item); if (!source) return null; const id = string(source.id); const name = string(source.name); const nodeCount = number(source.nodeCount); const url = source.url === undefined ? null : nullableString(source.url); if (id === null || name === null || nodeCount === null || url === undefined) return null; result.push({ id, name, nodeCount, url }); }
  return result;
}
function parseNodes(value: unknown): Node[] | null {
  const values = array(value); if (!values) return null; const result: Node[] = [];
  for (const item of values) { const node = record(item); if (!node) return null; const name = string(node.name); const protocol = string(node.protocol); const source = string(node.source); const available = typeof node.available === 'boolean' ? node.available : null; const error = nullableString(node.error); const delayMs = node.delayMs === null ? null : number(node.delayMs); if (name === null || protocol === null || source === null || available === null || error === undefined || delayMs === undefined) return null; result.push({ name, protocol, source, available, error, delayMs }); }
  return result;
}
function parseMember(value: unknown): Member | null { const item = record(value); if (!item) return null; const name = string(item.name); const kind = item.kind === 'node' || item.kind === 'group' ? item.kind : null; const available = typeof item.available === 'boolean' ? item.available : null; const delayMs = item.delayMs === null ? null : number(item.delayMs); return name !== null && kind !== null && available !== null && delayMs !== undefined ? { name, kind, available, delayMs } : null; }
function parseGroups(value: unknown): Group[] | null {
  const values = array(value); if (!values) return null; const result: Group[] = [];
  for (const item of values) { const group = record(item); if (!group) return null; const id = string(group.id); const name = string(group.name); const kind: GroupKind | null = group.kind === 'select' || group.kind === 'fallback' || group.kind === 'url-test' ? group.kind : null; const selected = string(group.selected); const resolved = nullableString(group.resolved); const members = array(group.members); if (id === null || name === null || kind === null || selected === null || resolved === undefined || !members) return null; const parsedMembers: Member[] = []; for (const member of members) { const parsed = parseMember(member); if (!parsed) return null; parsedMembers.push(parsed); } result.push({ id, name, kind, selected, resolved, members: parsedMembers }); }
  return result;
}
function parseRules(value: unknown): Rule[] | null { const values = array(value); if (!values) return null; const result: Rule[] = []; for (const item of values) { const rule = record(item); if (!rule) return null; const id = string(rule.id); const name = string(rule.name); const kind = string(rule.kind); const parsedValue = string(rule.value); const via = string(rule.via); if (id === null || name === null || kind === null || parsedValue === null || via === null) return null; result.push({ id, name, kind, value: parsedValue, via }); } return result; }
function parseConnection(value: unknown): Connection | null { const item = record(value); if (!item) return null; const id = string(item.id); const active = typeof item.active === 'boolean' ? item.active : null; const host = string(item.host); const port = number(item.port); const network = string(item.network); const outbound = string(item.outbound); const rule = string(item.rule); const chain = parseStrings(item.chain); const uploadBytes = number(item.uploadBytes); const downloadBytes = number(item.downloadBytes); const startedAt = number(item.startedAt); return id !== null && active !== null && host !== null && port !== null && network !== null && outbound !== null && rule !== null && chain !== null && uploadBytes !== null && downloadBytes !== null && startedAt !== null ? { id, active, host, port, network, outbound, rule, chain, uploadBytes, downloadBytes, startedAt } : null; }
function parseRuntime(value: unknown): Runtime | null { const item = record(value); if (!item) return null; const phase = item.phase === 'disconnected' || item.phase === 'connecting' || item.phase === 'connected' || item.phase === 'disconnecting' || item.phase === 'error' ? item.phase : null; const message = nullableString(item.message); const connectedAt = item.connectedAt === null ? null : number(item.connectedAt); const uploadBytes = number(item.uploadBytes); const downloadBytes = number(item.downloadBytes); const values = array(item.connections); if (phase === null || message === undefined || connectedAt === undefined || uploadBytes === null || downloadBytes === null || !values) return null; const connections: Connection[] = []; for (const value of values) { const parsed = parseConnection(value); if (!parsed) return null; connections.push(parsed); } return { phase, message, connectedAt, uploadBytes, downloadBytes, connections }; }

export async function request(operation: Record<string, unknown>): Promise<Snapshot> {
  const native = getBridge();
  if (!native) throw new BridgeError('原生运行模块尚未接入。请使用带 VPN 模块的安装包。');
  let raw: string;
  try { raw = await native.request(JSON.stringify(operation)); } catch { throw new BridgeError('无法联系代理运行模块，请稍后重试。', 'bridge_error'); }
  let parsed: unknown;
  try { parsed = JSON.parse(raw); } catch { throw new BridgeError('运行模块返回了无法识别的数据。', 'invalid_response'); }
  const envelope = record(parsed);
  if (!envelope || typeof envelope.ok !== 'boolean') throw new BridgeError('运行模块返回了无法识别的数据。', 'invalid_response');
  if (!envelope.ok) { const error = record(envelope.error); const message = error ? string(error.message) : null; const code = error ? string(error.code) : null; throw new BridgeError(message ?? '运行模块拒绝了这次操作。', code ?? 'native_error'); }
  const snapshot = parseSnapshot(envelope.data);
  if (!snapshot) throw new BridgeError('运行模块没有返回完整的当前状态。', 'invalid_response');
  return snapshot;
}

export async function send(operation: Record<string, unknown>): Promise<Snapshot> {
  return request(operation);
}
