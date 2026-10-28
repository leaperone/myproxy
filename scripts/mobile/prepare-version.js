const fs = require('node:fs');
const path = require('node:path');

const root = path.resolve(__dirname, '../..');
const manifest = fs.readFileSync(path.join(root, 'Cargo.toml'), 'utf8');
const version = /^version\s*=\s*"(\d+\.\d+\.\d+)"/m.exec(manifest)?.[1];
const run = Number(process.env.GITHUB_RUN_NUMBER ?? 1);
const attempt = Number(process.env.GITHUB_RUN_ATTEMPT ?? 1);
if (!version || !Number.isSafeInteger(run) || run < 1 || !Number.isSafeInteger(attempt) || attempt < 1 || attempt > 99 || run * 100 + attempt > 2100000000) {
  throw new Error('Invalid mobile build version');
}
const filename = path.join(root, 'mobile/app/app.json');
const config = JSON.parse(fs.readFileSync(filename, 'utf8'));
config.expo.version = version;
config.expo.ios.buildNumber = `${run}.${attempt}`;
config.expo.android.versionCode = run * 100 + attempt;
fs.writeFileSync(filename, `${JSON.stringify(config, null, 2)}\n`);
process.stdout.write(`MyProxy Xray ${version}, build ${run}.${attempt}\n`);
