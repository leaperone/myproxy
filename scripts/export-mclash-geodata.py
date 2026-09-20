#!/usr/bin/env python3
"""Copy selected installed MClash routing databases into a private MyProxy snapshot."""
from pathlib import Path
import json, ipaddress, hashlib, os, argparse
os.umask(0o077)
def varint(data, at):
 value=shift=0
 while True:
  if at>=len(data) or shift>63:raise ValueError('invalid protobuf varint')
  byte=data[at];at+=1;value|=(byte&127)<<shift
  if not byte&128:return value,at
  shift+=7
def fields(data):
 at=0
 while at<len(data):
  key,at=varint(data,at);field=key>>3;kind=key&7
  if not field:raise ValueError('invalid protobuf field')
  if kind==0:value,at=varint(data,at)
  elif kind==2:
   size,at=varint(data,at);value=data[at:at+size];at+=size
   if len(value)!=size:raise ValueError('truncated protobuf')
  elif kind in (1,5):
   size=8 if kind==1 else 4;value=data[at:at+size];at+=size
   if len(value)!=size:raise ValueError('truncated protobuf')
  else:raise ValueError('unsupported protobuf wire type')
  yield field,value
parser=argparse.ArgumentParser(description=__doc__)
parser.add_argument('--source-dir',type=Path,default=Path('/Applications/MClash.app/Contents/Resources/GeoData'))
parser.add_argument('--output',type=Path,default=Path.home()/'Library/Application Support/myproxy-xray/rulesets/mclash-geodata.json')
args=parser.parse_args()
geo=args.source_dir
result={'schema':1,'sites':{},'ips':{},'source_sha256':{}}
for file in ('geosite.dat','geoip.dat'):
 raw=(geo/file).read_bytes();result['source_sha256'][file]=hashlib.sha256(raw).hexdigest()
 for fid,entry in fields(raw):
  if fid!=1:continue
  content=list(fields(entry));country=next(v.decode().lower() for f,v in content if f==1)
  wanted={'private','cn','gfw','category-ads-all'} if file=='geosite.dat' else {'private','cn'}
  if country not in wanted:continue
  records=[]
  for f,record in content:
   if f!=2:continue
   item=dict(fields(record))
   if file=='geosite.dat':records.append({'kind':{0:'keyword',1:'regex',2:'suffix',3:'domain'}[item.get(1,0)],'value':item[2].decode()})
   else:
    address=ipaddress.ip_address(item[1]);records.append(str(ipaddress.ip_network((address,item.get(2,0)),strict=False)))
  if file=='geoip.dat' and any(f==3 and v for f,v in content):raise ValueError('unexpected reverse IP set')
  result['sites' if file=='geosite.dat' else 'ips'][country]=records
assert result['sites'].keys()=={'private','cn','gfw','category-ads-all'}
assert result['ips'].keys()=={'private','cn'}
output=args.output
output.parent.mkdir(parents=True,exist_ok=True,mode=0o700)
output.write_text(json.dumps(result,ensure_ascii=False,separators=(',',':')));output.chmod(0o600)
for kind in ('sites','ips'):
 print(kind,{name:len(entries) for name,entries in result[kind].items()})
print('Protected local routing data written:',output.stat().st_size,'bytes')
