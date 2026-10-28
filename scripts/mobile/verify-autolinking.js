const { execFileSync } = require('node:child_process');

const platform = process.argv[2];
if (!['apple', 'android'].includes(platform)) throw new Error('Unknown native platform');
const resolved = JSON.parse(execFileSync('npx', ['expo-modules-autolinking', 'resolve', '--platform', platform, '--json'], { encoding: 'utf8' }));
const expected = platform === 'apple' ? 'MyProxyModule' : 'one.leaper.myproxy.MyProxyModule';
function registered(value) {
  if (value === expected) return true;
  if (Array.isArray(value)) return value.some(registered);
  if (value && typeof value === 'object') return Object.values(value).some(registered);
  return false;
}
if (!registered(resolved)) throw new Error(`MyProxy native module was not registered for ${platform}`);
process.stdout.write(`MyProxy native module registered for ${platform}\n`);
