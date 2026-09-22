#!/usr/bin/env python3
"""Safely replace the signed Xray app and restore its previous connection."""
import atexit, datetime, hashlib, json, os, plistlib, shutil, subprocess, sys, tempfile, time
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
def cli_run(cli_path,*args,timeout=65):
 p=subprocess.run([str(cli_path),'--json',*args],check=True,capture_output=True,text=True,timeout=timeout)
 return json.loads(p.stdout)
def run(*args,timeout=65):
 return cli_run(cli,*args,timeout=timeout)
def verify_proxy_clients(port):
 for mode in ('http','socks'):
  proxy_args=['--proxy',f'http://127.0.0.1:{port}'] if mode=='http' else ['--socks5-hostname',f'127.0.0.1:{port}']
  response=subprocess.run(['/usr/bin/curl','-q','-sS',*proxy_args,'--noproxy','','--max-time','12','-o','/dev/null','-w','%{http_code}','https://www.gstatic.com/generate_204'],capture_output=True,text=True,timeout=15)
  assert response.returncode==0 and response.stdout=='204', f'{mode} proxy did not pass the post-upgrade network check'
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
atexit.register(lambda: shutil.rmtree(stage, ignore_errors=True))
subprocess.run(['ditto','-x','-k',str(archive),str(stage)],check=True)
candidate=stage/'MyProxy.app'
subprocess.run(['codesign','--verify','--deep','--strict',str(candidate)],check=True)
print('Validated candidate, exported current settings, backup:',backup,flush=True)
run('disconnect')
stopped=run('status')
extension=stopped.get('extension_runtime',{})
assert not stopped['xray']['running'] and not stopped['xray']['wanted'] and extension.get('observed') and not extension.get('captureEnabled') and extension.get('phase')=='disabled' and extension.get('dnsPhase')=='disabled', 'disconnect did not confirm Xray, capture, and DNS stopped'
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
 subprocess.run(['/usr/bin/ditto',str(backup/old.name),str(old)],check=True)
 subprocess.run(['codesign','--verify','--deep','--strict',str(old)],check=True)
 restored_cli=old/'Contents/MacOS/myproxyctl'
 subprocess.run(['/usr/bin/open',str(old),'--args','--host-control'],check=True)
 if was_connected:
  cli_run(restored_cli,'connect')
  restored=cli_run(restored_cli,'status')
  assert restored['xray']['running'] and restored['xray']['ready'] and restored['mixed_port']==before['mixed_port']
  verify_proxy_clients(before['mixed_port'])
 raise
receipt={'backup':str(backup),'application':str(destination),'archive_sha256':hashlib.sha256(archive.read_bytes()).hexdigest(),'previous_application':str(old),'previous_version':info['CFBundleShortVersionString']}
receipt['restore_connection']=was_connected
(backup/'upgrade-receipt.json').write_text(json.dumps(receipt,ensure_ascii=False,indent=2))
cli=destination/'Contents/MacOS/myproxyctl'

def exact_process_alive(executable):
 rows=subprocess.check_output(['ps','-axo','pid=,comm='],text=True).splitlines()
 return any(line.strip().split(None,1)[-1]==str(executable) for line in rows)

def wait_for_exact_process_exit(executable, timeout=15):
 deadline=time.monotonic()+timeout
 while exact_process_alive(executable):
  if time.monotonic() >= deadline: return False
  time.sleep(.2)
 return True


def recover_replaced_application(previous_bundle, candidate_bundle, destination, was_connected, previous_port):
 """Restore the old bundle after a post-replacement failure.

 The candidate is removed only after its exact executable is confirmed gone.
 No strategy or extension preference is changed here.
 """
 candidate_exe=candidate_bundle/'Contents/MacOS/myproxy'
 previous_info=plistlib.loads((previous_bundle/'Contents/Info.plist').read_bytes())
 assert previous_info.get('CFBundleIdentifier') in ['local.harry.myproxy','one.leaper.myproxy.xray-test']
 subprocess.run(['codesign','--verify','--deep','--strict',str(previous_bundle)],check=True)
 candidate_cli=candidate_bundle/'Contents/MacOS/myproxyctl'
 cli_run(candidate_cli,'disconnect')
 stopped=cli_run(candidate_cli,'status')
 extension=stopped.get('extension_runtime',{})
 if stopped.get('xray',{}).get('running') or stopped.get('xray',{}).get('wanted') or not extension.get('observed') or extension.get('captureEnabled') or extension.get('phase') != 'disabled' or extension.get('dnsPhase') != 'disabled':
  raise RuntimeError('candidate disconnect did not confirm Xray, capture, and DNS stopped')
 candidate_info=plistlib.loads((candidate_bundle/'Contents/Info.plist').read_bytes())
 assert candidate_info.get('CFBundleIdentifier') in ['local.harry.myproxy','one.leaper.myproxy.xray-test']
 subprocess.run(['/usr/bin/osascript','-e',f'tell application "{candidate_bundle}" to quit'],check=True,capture_output=True,text=True,timeout=15)
 if not wait_for_exact_process_exit(candidate_exe):
  raise RuntimeError(f'candidate application did not quit; left running at {candidate_bundle}')
 if candidate_bundle.exists(): shutil.rmtree(candidate_bundle)
 if not previous_bundle.exists(): raise RuntimeError(f'previous backup bundle is missing at {previous_bundle}')
 subprocess.run(['/usr/bin/ditto',str(previous_bundle),str(destination)],check=True)
 subprocess.run(['codesign','--verify','--deep','--strict',str(destination)],check=True)
 restored_cli=destination/'Contents/MacOS/myproxyctl'
 subprocess.run(['/usr/bin/open',str(destination),'--args','--host-control'],check=True)
 if was_connected:
  cli_run(restored_cli,'connect')
  restored=cli_run(restored_cli,'status')
  assert restored['xray']['running'] and restored['xray']['ready'], 'restored proxy did not become ready'
  assert restored['mixed_port']==previous_port, 'restored proxy port changed'
  verify_proxy_clients(previous_port)

try:
 subprocess.run(['/usr/bin/open',str(destination),'--args','--host-control'],check=True)
 if was_connected:
  print('Restoring the connection that was active before upgrade.',flush=True)
  restored=run('connect')
  restored=run('status')
  assert restored['xray']['running'] and restored['xray']['ready'], 'Proxy did not become ready after upgrade'
  assert restored['mixed_port']==before['mixed_port'], 'Proxy port changed during upgrade'
  verify_proxy_clients(before['mixed_port'])
  receipt['connection_restored']=True
 else:
  receipt['connection_restored']=False
 (backup/'upgrade-receipt.json').write_text(json.dumps(receipt,ensure_ascii=False,indent=2))
 print('Signed MyProxy installed; configuration and previous connection state preserved.',flush=True)
except Exception as failure:
 try:
  recover_replaced_application(backup/old.name,destination,destination,was_connected,before['mixed_port'])
 except Exception as recovery:
  raise RuntimeError(f'upgrade verification failed; recovery blocked: {recovery}') from failure
 receipt['recovered_previous_application']=True
 receipt['recovery_error']=str(failure)
 (backup/'upgrade-receipt.json').write_text(json.dumps(receipt,ensure_ascii=False,indent=2))
 raise RuntimeError(f'upgrade verification failed; previous application restored and recovery verified: {failure}') from failure
