export type RuntimePhase = 'disconnected' | 'connecting' | 'connected' | 'disconnecting' | 'error';
export type GroupKind = 'select' | 'fallback' | 'url-test';

export type Source = { id: string; name: string; url?: string | null; nodeCount: number };
export type Node = {
  name: string;
  protocol: string;
  source: string;
  available: boolean;
  error: string | null;
  delayMs: number | null;
};
export type Member = {
  name: string;
  kind: 'node' | 'group';
  available: boolean;
  delayMs: number | null;
};
export type Group = {
  id: string;
  name: string;
  kind: GroupKind;
  selected: string;
  resolved: string | null;
  members: Member[];
};
export type Rule = { id: string; name: string; kind: string; value: string; via: string };
export type Connection = {
  id: string;
  active: boolean;
  host: string;
  port: number;
  network: string;
  outbound: string;
  rule: string;
  chain: string[];
  uploadBytes: number;
  downloadBytes: number;
  startedAt: number;
};
export type Runtime = {
  phase: RuntimePhase;
  message: string | null;
  connectedAt: number | null;
  uploadBytes: number;
  downloadBytes: number;
  connections: Connection[];
};
export type Snapshot = {
  revision: number;
  mode: 'global' | 'rule';
  autoConnect: boolean;
  selected: string;
  fallback: string;
  capabilities: { dynamicDirect: boolean; appRouting: boolean };
  sources: Source[];
  nodes: Node[];
  groups: Group[];
  rules: Rule[];
  warnings: string[];
  runtime: Runtime;
};

export const EMPTY_SNAPSHOT: Snapshot = {
  revision: 0,
  mode: 'rule',
  autoConnect: false,
  selected: '',
  fallback: '',
  capabilities: { dynamicDirect: false, appRouting: false },
  sources: [],
  nodes: [],
  groups: [],
  rules: [],
  warnings: [],
  runtime: { phase: 'disconnected', message: null, connectedAt: null, uploadBytes: 0, downloadBytes: 0, connections: [] },
};

export function formatBytes(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`;
  if (bytes < 1024 * 1024 * 1024) return `${(bytes / (1024 * 1024)).toFixed(1)} MB`;
  return `${(bytes / (1024 * 1024 * 1024)).toFixed(2)} GB`;
}
