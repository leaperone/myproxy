#!/usr/bin/env python3
"""Prepare or apply a private, repeatable MClash rule migration via myproxyctl."""
import argparse
import copy
import datetime
import hashlib
import json
import os
from pathlib import Path
import subprocess
import uuid


def identifier(value):
    return (value['rawValue'] if isinstance(value, dict) else value).lower()


def run(cli, *arguments):
    result = subprocess.run([str(cli), '--json', *arguments], capture_output=True, text=True, timeout=90)
    if result.returncode:
        raise RuntimeError('MyProxy CLI rejected the operation; the source configuration was not changed')
    return json.loads(result.stdout)


def action_target(action, groups):
    if 'direct' in action:
        return 'DIRECT'
    if 'reject' in action:
        return 'REJECT'
    return groups[identifier(action['proxyGroup']['_0'])]


def rule_name(matchers, position):
    values = ' '.join(item['value'] for item in matchers).lower()
    for key, title in [('cursor', 'Cursor'), ('claude', 'Claude'), ('telegram', 'Telegram'), ('chatgpt', 'ChatGPT'), ('openai', 'OpenAI'), ('chrome', 'Chrome'), ('firefox', 'Firefox'), ('safari', 'Safari'), ('github', 'GitHub'), ('google', 'Google')]:
        if key in values and len(matchers) < 100:
            return f'{title} · {position}'
    first = next(item for item in matchers if item['kind'] != 'network')
    if first['kind'] == 'cidr':
        return f'网络直连 · {first["value"]}'
    if first['kind'] == 'app':
        return f'应用分流 · {first["value"].strip("*")} · {position}'
    return f'域名分流 · {position}'


def prepare(source, current):
    candidate = copy.deepcopy(current)
    groups = {}
    names = {group['name'] for group in current['groups']}
    for group in source['proxyGroups']:
        target = next((name for name in names if group['name'].endswith(name)), None)
        if group['type'] == 'direct':
            target = 'DIRECT'
        if group['type'] == 'reject':
            target = 'REJECT'
        if target:
            groups[identifier(group['id'])] = target
    workspace = next(item for item in source['workspaces'] if identifier(item['id']) == identifier(source['currentWorkspaceID']))
    active = {identifier(value) for value in workspace['ruleIDs']}
    existing = {rule['id']: rule for rule in candidate['rule_sets']}
    migrated = []
    replaced = []
    unsupported = []
    kind_map = {'application': 'app', 'processPath': 'app', 'processName': 'app', 'domainExact': 'domain', 'domainSuffix': 'suffix', 'domainWildcard': 'wildcard', 'ipCIDR': 'cidr', 'transport': 'network'}
    for position, rule in enumerate(sorted(source['rules'], key=lambda item: item['priority'])):
        source_id = identifier(rule['id'])
        if source_id not in active or not rule['enabled']:
            continue
        kinds = {next(iter(item)) for item in rule['matchers']}
        # A former app's UID+loopback-DNS bypass is not a user route. The new
        # signed host/core identities have their own built-in recursion bypass.
        if kinds == {'userID', 'ipCIDR', 'port'} and 'direct' in rule['action'] and rule['priority'] < 0:
            ports = [item['port']['_0'] for item in rule['matchers'] if 'port' in item]
            if ports == [53]:
                replaced.append({'source_id': source_id, 'reason': '旧应用的 DNS 自保护由 MyProxy 签名组件旁路替代'})
                continue
        if not kinds <= kind_map.keys():
            unsupported.append({'source_id': source_id, 'matcher_types': sorted(kinds)})
            continue
        if kinds & {'application', 'processPath', 'processName'} and kinds & {'domainExact', 'domainSuffix', 'domainWildcard', 'ipCIDR'}:
            unsupported.append({'source_id': source_id, 'reason': '组合了应用与目标条件，需要保留 AND 语义'})
            continue
        matchers = [{'kind': kind_map[next(iter(item))], 'value': str(next(iter(item.values()))['_0'])} for item in rule['matchers']]
        rule_id = str(uuid.uuid5(uuid.NAMESPACE_URL, 'mclash-rule:' + source_id))
        entry = existing.pop(rule_id, None)
        if entry is None:
            entry = {'id': rule_id, 'name': rule_name(matchers, position), 'via': action_target(rule['action'], groups), 'matchers': matchers}
        entry.setdefault('unavailable_fallback', rule.get('unavailableFallback', 'reject'))
        migrated.append(entry)
    rule_sets = {identifier(item['id']): item for item in source['ruleSets']}
    for source_id in workspace.get('ruleSetIDs', []):
        rule_set = rule_sets[identifier(source_id)]
        if not rule_set.get('enabled', True):
            continue
        matchers = []
        for raw in rule_set['rules']:
            parts = [part.strip() for part in raw.split(',')]
            if len(parts) < 2 or parts[0] not in ('GEOSITE', 'GEOIP', 'GEOIP6'):
                unsupported.append({'source_id': identifier(source_id), 'reason': '未识别的规则集条目'})
                continue
            matchers.append({'kind': 'geo-site' if parts[0] == 'GEOSITE' else 'geo-ip', 'value': parts[1].lower()})
        if not matchers:
            continue
        rule_id = str(uuid.uuid5(uuid.NAMESPACE_URL, 'mclash-ruleset:' + identifier(source_id)))
        migrated.append(existing.pop(rule_id, None) or {'id': rule_id, 'name': rule_set['name'], 'via': action_target(rule_set['defaultAction'], groups), 'matchers': matchers})
    # Preserve unrelated user rules and their relative order ahead of imports.
    candidate['rule_sets'] = list(existing.values()) + migrated
    if unsupported:
        raise RuntimeError('Some rules cannot yet be represented without changing behavior; migration was not applied')
    return candidate, {'imported_rules': len(migrated), 'preserved_user_rules': len(existing), 'replaced_internal_rules': replaced}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--cli', type=Path, default=Path('/Applications/MyProxy.app/Contents/MacOS/myproxyctl'))
    parser.add_argument('--source', type=Path, default=Path.home() / 'Library/Application Support/MClash/Configuration/manifest.json')
    parser.add_argument('--data-dir', type=Path, default=Path.home() / 'Library/Application Support/myproxy-xray')
    parser.add_argument('--apply', action='store_true')
    args = parser.parse_args()
    os.umask(0o077)
    backup = args.data_dir / 'Backups' / ('rules-migration-' + datetime.datetime.now().strftime('%Y%m%d-%H%M%S'))
    backup.mkdir(parents=True, mode=0o700)
    run(args.cli, 'export', str(backup / 'before.json'))
    current = json.loads((backup / 'before.json').read_text())
    source_bytes = args.source.read_bytes()
    candidate, receipt = prepare(json.loads(source_bytes), current)
    for key in ('mixed_port', 'mixed_mode', 'global_selected', 'subscriptions', 'groups', 'system_extension', 'system_proxy'):
        assert candidate[key] == current[key], 'migration changed unrelated settings'
    candidate_file = backup / 'candidate.json'
    candidate_file.write_text(json.dumps(candidate, ensure_ascii=False, indent=2))
    receipt['source_sha256'] = hashlib.sha256(source_bytes).hexdigest()
    receipt['applied'] = False
    if args.apply:
        backend = run(args.cli, 'backend')
        assert backend['backend'] == 'xray', 'Xray channel required'
        run(args.cli, 'import', str(candidate_file))
        run(args.cli, 'apply')
        receipt['applied'] = True
        run(args.cli, 'export', str(backup / 'after.json'))
    (backup / 'receipt.json').write_text(json.dumps(receipt, ensure_ascii=False, indent=2))
    print(json.dumps({'rules': receipt['imported_rules'], 'preserved': receipt['preserved_user_rules'], 'replaced_internal': len(receipt['replaced_internal_rules']), 'applied': receipt['applied'], 'backup': str(backup)}, ensure_ascii=False))


if __name__ == '__main__':
    main()
