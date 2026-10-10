import { spawn } from 'node:child_process';
import { randomUUID } from 'node:crypto';
import { createReadStream } from 'node:fs';
import { access, stat } from 'node:fs/promises';
import { createServer } from 'node:http';
import { dirname, extname, join, normalize, relative } from 'node:path';
import { Readable } from 'node:stream';
import { getAsset, isSea } from 'node:sea';
import { fileURLToPath } from 'node:url';
import { FavoritesStore, invalid } from './favorites-store.mjs';

const HOST = '127.0.0.1';
const DEFAULT_PORT = 4180;
const TOKEN_TTL_MS = 30 * 60 * 1000;
const REQUEST_TIMEOUT_MS = 15_000;
const BILIBILI_REFERER = 'https://www.bilibili.com/';
const USER_AGENT =
  'Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 ' +
  '(KHTML, like Gecko) Chrome/124.0.0.0 Safari/537.36';

const packaged = Boolean(process.pkg) || isSea();
const rootDir = packaged ? dirname(process.execPath) : dirname(fileURLToPath(import.meta.url));
// 单文件程序的内嵌资源只读；用户数据必须写在 exe 旁边。
const dataDir = packaged ? dirname(process.execPath) : rootDir;
const publicDir = join(rootDir, 'public');
const streams = new Map();

const MIME_TYPES = {
  '.css': 'text/css; charset=utf-8',
  '.html': 'text/html; charset=utf-8',
  '.js': 'text/javascript; charset=utf-8',
  '.json': 'application/json; charset=utf-8',
  '.svg': 'image/svg+xml',
};

export function extractBvid(input) {
  if (typeof input !== 'string') return null;
  const match = input.trim().match(/(?:^|\/)(BV[0-9A-Za-z]{10})(?:[/?#]|$)/i);
  return match ? `BV${match[1].slice(2)}` : null;
}

export function normalizeCoverUrl(url) {
  if (typeof url !== 'string' || !url.trim()) return '';
  if (url.startsWith('//')) return `https:${url}`;
  return url.replace(/^http:\/\//i, 'https://');
}

export function chooseAudioTrack(playData) {
  const tracks = [
    ...(Array.isArray(playData?.dash?.audio) ? playData.dash.audio : []),
    ...(Array.isArray(playData?.dash?.dolby?.audio) ? playData.dash.dolby.audio : []),
    ...(playData?.dash?.flac?.audio ? [playData.dash.flac.audio] : []),
  ].filter((track) => track?.baseUrl || track?.base_url);
  return tracks.sort((a, b) => Number(b.bandwidth ?? 0) - Number(a.bandwidth ?? 0))[0] ?? null;
}

export function extractCollection(videoData) {
  const season = videoData?.ugc_season;
  if (!season || !Array.isArray(season.sections)) return null;

  const sections = season.sections
    .map((section, sectionIndex) => ({
      id: section.id ?? section.section_id ?? sectionIndex,
      title: section.title || '',
      episodes: (Array.isArray(section.episodes) ? section.episodes : [])
        .map((episode) => ({
          bvid: episode.bvid || episode.arc?.bvid || '',
          title: episode.title || episode.arc?.title || episode.bvid || '',
          duration: episode.page?.duration ?? episode.arc?.duration ?? null,
          coverUrl: normalizeCoverUrl(episode.arc?.pic || episode.cover || ''),
        }))
        .filter((episode) => episode.bvid),
    }))
    .filter((section) => section.episodes.length > 0);

  if (sections.length === 0) return null;
  return {
    id: season.id ?? season.season_id ?? null,
    title: season.title || '视频合集',
    sections,
  };
}

export function forwardableAudioHeaders(headers) {
  const result = {};
  for (const name of [
    'accept-ranges',
    'cache-control',
    'content-length',
    'content-range',
    'content-type',
    'etag',
    'last-modified',
  ]) {
    const value = headers.get(name);
    if (value) result[name] = value;
  }
  return result;
}

function apiHeaders(extra = {}) {
  return {
    Accept: 'application/json, text/plain, */*',
    Referer: BILIBILI_REFERER,
    'User-Agent': USER_AGENT,
    ...extra,
  };
}

async function fetchWithConnectionTimeout(url, options = {}) {
  const controller = new AbortController();
  const timer = setTimeout(() => controller.abort(), REQUEST_TIMEOUT_MS);
  try {
    return await fetch(url, { ...options, signal: controller.signal });
  } finally {
    clearTimeout(timer);
  }
}

async function fetchJson(url) {
  const response = await fetchWithConnectionTimeout(url, { headers: apiHeaders() });
  if (!response.ok) throw new Error(`Bilibili 请求失败（HTTP ${response.status}）`);
  return response.json();
}

export async function fetchVideoMetadata(input) {
  const bvid = extractBvid(input);
  if (!bvid) throw invalid('请输入有效的 BV 号或 Bilibili 视频链接');

  const viewUrl = new URL('https://api.bilibili.com/x/web-interface/view');
  viewUrl.searchParams.set('bvid', bvid);
  const view = await fetchJson(viewUrl);
  if (view.code !== 0 || !view.data) {
    throw new Error(view.message || '视频不存在或当前无法访问');
  }

  const page = view.data.pages?.[0];
  const cid = page?.cid ?? view.data.cid;
  if (!cid) throw new Error('未找到视频分 P 信息');

  return { bvid: view.data.bvid || bvid, cid, title: view.data.title || bvid,
    owner: view.data.owner?.name || '', pageTitle: page?.part || '',
    duration: page?.duration ?? view.data.duration ?? null,
    coverUrl: normalizeCoverUrl(view.data.pic || ''), collection: extractCollection(view.data) };
}

export async function resolveBvid(input) {
  const metadata = await fetchVideoMetadata(input);
  const { bvid, cid } = metadata;

  const playUrl = new URL('https://api.bilibili.com/x/player/playurl');
  playUrl.searchParams.set('bvid', bvid);
  playUrl.searchParams.set('cid', String(cid));
  playUrl.searchParams.set('fnval', '16');
  playUrl.searchParams.set('fnver', '0');
  playUrl.searchParams.set('fourk', '1');
  const play = await fetchJson(playUrl);
  if (play.code !== 0 || !play.data) {
    throw new Error(play.message || '无法取得播放信息');
  }

  const audio = chooseAudioTrack(play.data);
  if (!audio) throw new Error('该视频没有可用的 DASH 音轨');

  const urls = [audio.baseUrl || audio.base_url, ...(audio.backupUrl || audio.backup_url || [])].filter(
    Boolean,
  );
  const token = randomUUID();
  streams.set(token, { urls, expiresAt: Date.now() + TOKEN_TTL_MS });

  return {
    ...metadata,
    audioUrl: `/api/audio/${token}`,
  };
}

function sendJson(response, statusCode, data) {
  const body = JSON.stringify(data);
  response.writeHead(statusCode, {
    'Content-Type': MIME_TYPES['.json'],
    'Content-Length': Buffer.byteLength(body),
    'Cache-Control': 'no-store',
  });
  response.end(body);
}

async function readJsonBody(request, limit = 16 * 1024) {
  const chunks = [];
  let size = 0;
  for await (const chunk of request) {
    size += chunk.length;
    if (size > limit) throw invalid('请求内容过大');
    chunks.push(chunk);
  }
  try {
    return JSON.parse(Buffer.concat(chunks).toString('utf8') || '{}');
  } catch {
    throw invalid('请求内容不是有效 JSON');
  }
}

async function handleResolve(request, response) {
  try {
    const body = await readJsonBody(request);
    sendJson(response, 200, await resolveBvid(body.input));
  } catch (error) {
    sendJson(response, 400, { error: error instanceof Error ? error.message : '解析失败' });
  }
}

async function requestAudio(urls, request) {
  const headers = apiHeaders({ Accept: '*/*' });
  if (request.headers.range) headers.Range = request.headers.range;
  if (request.headers['if-range']) headers['If-Range'] = request.headers['if-range'];

  let lastResponse;
  for (const url of urls) {
    try {
      const upstream = await fetchWithConnectionTimeout(url, {
        method: request.method,
        headers,
        redirect: 'follow',
      });
      lastResponse = upstream;
      if (upstream.ok) return upstream;
      await upstream.body?.cancel();
    } catch {
      // Try a backup CDN URL.
    }
  }
  return lastResponse;
}

async function handleAudio(request, response, token) {
  const entry = streams.get(token);
  if (!entry || entry.expiresAt <= Date.now()) {
    streams.delete(token);
    return sendJson(response, 404, { error: '播放地址已失效，请重新提交 BV 号' });
  }

  const upstream = await requestAudio(entry.urls, request);
  if (!upstream?.ok) {
    return sendJson(response, 502, { error: 'Bilibili 音频源暂时不可用，请重新解析' });
  }

  const responseHeaders = forwardableAudioHeaders(upstream.headers);
  if (!responseHeaders['content-type'] || responseHeaders['content-type'] === 'application/octet-stream') {
    responseHeaders['content-type'] = 'audio/mp4';
  }
  response.writeHead(upstream.status, responseHeaders);
  if (request.method === 'HEAD' || !upstream.body) return response.end();

  const body = Readable.fromWeb(upstream.body);
  request.on('aborted', () => body.destroy());
  body.on('error', () => {
    if (!response.destroyed) response.destroy();
  });
  body.pipe(response);
}

async function serveStatic(response, pathname) {
  const requested = decodeURIComponent(pathname === '/' ? 'index.html' : pathname.slice(1));
  if (isSea()) {
    try {
      const content = Buffer.from(getAsset(`public/${requested}`));
      response.writeHead(200, {
        'Content-Type': MIME_TYPES[extname(requested)] || 'application/octet-stream',
        'Content-Length': content.length,
        'Cache-Control': 'no-store',
      });
      return response.end(content);
    } catch {
      return sendJson(response, 404, { error: '页面不存在' });
    }
  }
  const filePath = normalize(join(publicDir, requested));
  const relativePath = relative(publicDir, filePath);
  if (relativePath.startsWith('..') || relativePath.includes(':')) {
    return sendJson(response, 403, { error: '禁止访问' });
  }

  try {
    await access(filePath);
    const details = await stat(filePath);
    if (!details.isFile()) throw new Error('not a file');
    response.writeHead(200, {
      'Content-Type': MIME_TYPES[extname(filePath)] || 'application/octet-stream',
      'Content-Length': details.size,
      'Cache-Control': 'no-store',
    });
    createReadStream(filePath).pipe(response);
  } catch {
    sendJson(response, 404, { error: '页面不存在' });
  }
}

export function createDemoServer({ favoritesDirectory = join(dataDir, 'favo'), metadataResolver = fetchVideoMetadata } = {}) {
  const favorites = new FavoritesStore(favoritesDirectory);
  return createServer(async (request, response) => {
    try {
      const url = new URL(request.url || '/', `http://${request.headers.host || HOST}`);
      if (url.pathname.startsWith('/api/favorites')) {
        const path = url.pathname;
        if (request.method === 'POST' && path === '/api/favorites/import') {
          const body = await readJsonBody(request, 10 * 1024 * 1024 + 1024);
          return sendJson(response, 200, await favorites.import(body.data, body.preview === true));
        }
        if (request.method === 'GET' && (path === '/api/favorites/export' || /^\/api\/favorites\/[^/]+\/export$/.test(path))) {
          const id = path.match(/^\/api\/favorites\/([^/]+)\/export$/)?.[1];
          let data;
          if (id) data = await favorites.get(id);
          else { const result = await favorites.list(); if (result.errors.length) throw new Error(result.errors.join('\n')); data = { schemaVersion: 1, type: 'bili-sound-favorites', folders: result.folders }; }
          response.setHeader('Content-Disposition', `attachment; filename="favorites${id ? '-' + id : ''}.json"`);
          return sendJson(response, 200, data);
        }
        if (path === '/api/favorites') {
          if (request.method === 'GET') return sendJson(response, 200, await favorites.list());
          if (request.method === 'POST') { const body = await readJsonBody(request); return sendJson(response, 201, await favorites.create(body.name, body.description)); }
        }
        const match = path.match(/^\/api\/favorites\/([^/]+)(\/items)?$/);
        if (match && request.method === 'GET' && !match[2]) return sendJson(response, 200, await favorites.get(match[1]));
        if (match?.[2] && request.method === 'POST') {
          await favorites.get(match[1]);
          const body = await readJsonBody(request);
          let metadata; try { metadata = await metadataResolver(body.input); } catch (error) { error.status ||= 502; throw error; }
          return sendJson(response, 200, await favorites.add(match[1], metadata));
        }
        return sendJson(response, 404, { error: '收藏夹接口不存在' });
      }
      if (request.method === 'POST' && url.pathname === '/api/resolve') {
        return await handleResolve(request, response);
      }
      const audioMatch = url.pathname.match(/^\/api\/audio\/([0-9a-f-]+)$/i);
      if ((request.method === 'GET' || request.method === 'HEAD') && audioMatch) {
        return await handleAudio(request, response, audioMatch[1]);
      }
      if (request.method === 'GET') return await serveStatic(response, url.pathname);
      sendJson(response, 405, { error: '不支持的请求方法' });
    } catch (error) {
      sendJson(response, error.status || 500, { error: error instanceof Error ? error.message : '服务器错误' });
    }
  });
}

function openBrowser(url) {
  let child;
  if (process.platform === 'win32') {
    child = spawn('cmd.exe', ['/c', 'start', '', url], { detached: true, stdio: 'ignore' });
  } else if (process.platform === 'darwin') {
    child = spawn('open', [url], { detached: true, stdio: 'ignore' });
  } else {
    child = spawn('xdg-open', [url], { detached: true, stdio: 'ignore' });
  }
  child.on('error', () => {});
  child.unref();
}

function cleanExpiredTokens() {
  const now = Date.now();
  for (const [token, entry] of streams) {
    if (entry.expiresAt <= now) streams.delete(token);
  }
}

const isMain = isSea() || (process.argv[1] && fileURLToPath(import.meta.url) === normalize(process.argv[1]));
if (isMain) {
  const configuredPort = Number.parseInt(process.env.BILI_DEMO_PORT || '', 10);
  const port = Number.isInteger(configuredPort) && configuredPort > 0 ? configuredPort : DEFAULT_PORT;
  const server = createDemoServer();
  server.listen(port, HOST, () => {
    const url = `http://${HOST}:${port}`;
    console.log(`Bilibili GUI 播放器已启动：${url}`);
    console.log('按 Ctrl+C 停止服务。');
    if (!process.argv.includes('--no-open')) openBrowser(url);
  });
  setInterval(cleanExpiredTokens, 60_000).unref();
}
