#!/usr/bin/env python3
"""Safely replace the signed Xray app and restore its previous connection."""
import datetime, hashlib, json, os, plistlib, shutil, subprocess, sys, tempfile, time
from pathlib import Path
os.umask(0o077)
archive=Path(sys.argv[1]).resolve()
repo=Path(__file__).resolve().parents[1]
subprocess.run([sys.executable,str(repo/'scripts/check-xray-package.py'),str(archive),str(archive.with_name('appcast.xml'))],check=True)
root=Path.home()/'Library/Application Support/myproxy-xray'
old=Path('/Applications/MyProxy.app')
if not old.is_dir(): old=Path('/Applications/MyProxy Xray.app')
assert old.is_dir()
info=plistlib.loads((old/'Contents/Info.plist').read_bytes())
assert info['CFBundleIdentifier'] in ['local.harry.myproxy','one.leaper.myproxy.xray-test']
cli=old/'Contents/MacOS/myproxyctl'
def run(*args,timeout=65):
 p=subprocess.run([str(cli),'--json',*args],check=True,capture_output=True,text=True,timeout=timeout)
 return json.loads(p.stdout)
before=run('status')
was_connected=before.get('xray',{}).get('wanted',False)
assert before['backend']=='xray' and before['mixed_port']==40808
backup=root/'Backups'/('signed-upgrade-'+datetime.datetime.now().strftime('%Y%m%d-%H%M%S'))
backup.mkdir(parents=True,mode=0o700)
run('export',str(backup/'strategy-before.json'))
for name in ['catalog.json','onboard.json']:
 p=root/name
 if p.is_file(): shutil.copy2(p,backup/name)
if (root/'rulesets').exists(): shutil.copytree(root/'rulesets',backup/'rulesets')
(backup/'status-before.json').write_text(json.dumps(before,ensure_ascii=False,indent=2))
stage=Path(tempfile.mkdtemp(prefix='myproxy-signed-stage-'))
subprocess.run(['ditto','-x','-k',str(archive),str(stage)],check=True)
candidate=stage/'MyProxy.app'
subprocess.run(['codesign','--verify','--deep','--strict',str(candidate)],check=True)
print('Validated candidate, exported current settings, backup:',backup,flush=True)
run('disconnect')
stopped=run('status')
assert not stopped['xray']['running'] and not stopped['xray']['wanted']
print('Existing proxy disconnected cleanly; quitting only its verified application.',flush=True)
subprocess.run(['/usr/bin/osascript','-e',f'tell application "{old}" to quit'],check=True,capture_output=True,text=True,timeout=15)
exe=str(old/'Contents/MacOS/myproxy')
for attempt in range(100):
 rows=subprocess.check_output(['ps','-axo','pid=,comm='],text=True).splitlines()
 if not any(line.strip().split(None,1)[-1]==exe for line in rows): break
 time.sleep(.2)
else: raise RuntimeError('Application did not quit; bundle left unchanged')
old.rename(backup/old.name)
destination=Path('/Applications/MyProxy.app')
assert not destination.exists()
try:
 subprocess.run(['ditto',str(candidate),str(destination)],check=True)
 subprocess.run(['codesign','--verify','--deep','--strict',str(destination)],check=True)
except Exception:
 if destination.exists(): shutil.rmtree(destination)
 (backup/old.name).rename(old)
 raise
receipt={'backup':str(backup),'application':str(destination),'archive_sha256':hashlib.sha256(archive.read_bytes()).hexdigest(),'previous_application':str(old),'previous_version':info['CFBundleShortVersionString']}
receipt['restore_connection']=was_connected
(backup/'upgrade-receipt.json').write_text(json.dumps(receipt,ensure_ascii=False,indent=2))
subprocess.run(['/usr/bin/open',str(destination),'--args','--host-control'],check=True)
cli=destination/'Contents/MacOS/myproxyctl'
if was_connected:
    print('Restoring the connection that was active before upgrade.',flush=True)
    try:
        run('connect')
    except subprocess.CalledProcessError:
        if before.get('extension_runtime', {}).get('captureEnabled', False):
            raise
        # An un-applied capture preference must not prevent restoring the
        # local proxy that was already working before the upgrade.
        run('extension','off')
        run('connect')
        receipt['capture_preference_disabled_to_restore_previous_runtime']=True
    restored=run('status')
    assert restored['xray']['running'] and restored['xray']['ready'], 'Proxy did not become ready after upgrade'
    assert restored['mixed_port']==before['mixed_port'], 'Proxy port changed during upgrade'
    for mode in ('http','socks'):
        proxy_args=['--proxy',f'http://127.0.0.1:{before["mixed_port"]}'] if mode=='http' else ['--socks5-hostname',f'127.0.0.1:{before["mixed_port"]}']
        response=subprocess.run(['/usr/bin/curl','-q','-sS',*proxy_args,'--noproxy','','--max-time','12','-o','/dev/null','-w','%{http_code}','https://www.gstatic.com/generate_204'],capture_output=True,text=True,timeout=15)
        assert response.returncode==0 and response.stdout=='204', f'{mode} proxy did not pass the post-upgrade network check'
    receipt['connection_restored']=True
    (backup/'upgrade-receipt.json').write_text(json.dumps(receipt,ensure_ascii=False,indent=2))
    print('Connection restored and HTTP/SOCKS verified on the original port.',flush=True)
else:
    receipt['connection_restored']=False
    (backup/'upgrade-receipt.json').write_text(json.dumps(receipt,ensure_ascii=False,indent=2))
print('Signed MyProxy installed; configuration and previous connection state preserved.',flush=True)
shutil.rmtree(stage)
