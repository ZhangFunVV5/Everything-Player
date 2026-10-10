import { copyFile, mkdir, readdir, rm, writeFile } from 'node:fs/promises';
import { spawn } from 'node:child_process';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { build } from 'esbuild';

const root = dirname(fileURLToPath(import.meta.url));
const buildDir = join(root, 'build');
const distDir = join(root, 'dist');
const bundle = join(buildDir, 'server.cjs');
const blob = join(buildDir, 'sea-prep.blob');
const executable = join(distDir, 'BiliSound.exe');

function run(command, args) {
  return new Promise((resolvePromise, reject) => {
    const child = spawn(command, args, { cwd: root, stdio: 'inherit', shell: false });
    child.once('error', reject);
    child.once('exit', code => code === 0 ? resolvePromise() : reject(new Error(`${command} 退出码 ${code}`)));
  });
}

await rm(buildDir, { recursive: true, force: true });
await mkdir(buildDir, { recursive: true });
await mkdir(distDir, { recursive: true });
await build({ entryPoints: [join(root, 'server.mjs')], outfile: bundle, bundle: true, platform: 'node', format: 'cjs', target: 'node24' });

const assets = {};
for (const name of await readdir(join(root, 'public'))) assets[`public/${name}`] = resolve(root, 'public', name);
await writeFile(join(buildDir, 'sea-config.json'), JSON.stringify({ main: bundle, output: blob, disableExperimentalSEAWarning: true, useSnapshot: false, useCodeCache: false, assets }, null, 2));
await run(process.execPath, ['--experimental-sea-config', join(buildDir, 'sea-config.json')]);
await copyFile(process.execPath, executable);
await run(process.execPath, [join(root, 'node_modules', 'postject', 'dist', 'cli.js'), executable, 'NODE_SEA_BLOB', blob, '--sentinel-fuse', 'NODE_SEA_FUSE_fce680ab2cc467b6e072b8b5df1996b2']);
console.log(`已生成：${executable}`);
