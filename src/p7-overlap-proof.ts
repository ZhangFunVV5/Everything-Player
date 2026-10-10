// @ts-nocheck
// Windows debug proof build only: exercises the production player and native commands.
(() => {
  if (window.__p7OverlapProofInstalled) return;
  window.__p7OverlapProofInstalled = true;

  const C_BV = 'BV1DAkgBJEBt';
  const A_BV = 'BV1Q541167Qg';
  const B_BV = 'BV1ZZZZZZZZZ';
  const TARGET_TIME = 2400;
  const root = document.createElement('section');
  root.style.cssText = 'position:fixed;right:12px;top:42px;z-index:2147483647;width:370px;padding:9px;background:#15151bee;color:white;border:1px solid #f578ab;font:12px monospace';
  const button = document.createElement('button');
  button.textContent = 'Run P7-T5 overlap proof';
  const output = document.createElement('pre');
  output.setAttribute('aria-label', 'P7-T5 overlap trace');
  output.style.cssText = 'white-space:pre-wrap;max-height:240px;overflow:auto;margin:6px 0 0';
  root.append(button, output);
  document.body.append(root);
  const log = (message) => {
    output.textContent += `${message}\n`;
    output.scrollTop = output.scrollHeight;
  };
  const waitFor = async (predicate, label, timeout = 30000) => {
    const start = performance.now();
    while (!predicate()) {
      if (performance.now() - start > timeout) throw new Error(`${label} timed out`);
      await new Promise((resolve) => setTimeout(resolve, 50));
    }
  };
  const submit = (bvid) => {
    document.querySelector('#bv-input').value = bvid;
    document.querySelector('#bv-form').dispatchEvent(new Event('submit', { bubbles: true, cancelable: true }));
  };

  function indexedOffset(prefix, seconds) {
    const view = new DataView(prefix.buffer, prefix.byteOffset, prefix.byteLength);
    let offset = 0;
    let sidx = -1;
    let sidxSize = 0;
    let moof = -1;
    while (offset + 8 <= prefix.length) {
      const size = view.getUint32(offset);
      if (size < 8 || offset + size > prefix.length) return null;
      const type = String.fromCharCode(...prefix.subarray(offset + 4, offset + 8));
      if (type === 'sidx') { sidx = offset; sidxSize = size; }
      if (type === 'moof') { moof = offset; break; }
      offset += size;
    }
    if (sidx < 0 || moof < 0) return null;
    const version = view.getUint8(sidx + 8);
    const timescale = view.getUint32(sidx + 16);
    if (!timescale || (version !== 0 && version !== 1)) return null;
    let cursor = sidx + 20;
    const earliest = version ? Number(view.getBigUint64(cursor)) : view.getUint32(cursor);
    cursor += version ? 8 : 4;
    const firstOffset = version ? Number(view.getBigUint64(cursor)) : view.getUint32(cursor);
    cursor += version ? 8 : 4;
    cursor += 2;
    const count = view.getUint16(cursor);
    cursor += 2;
    if (!count || cursor + count * 12 > sidx + sidxSize) return null;
    let byte = sidx + sidxSize + firstOffset;
    if (byte !== moof) return null;
    let time = earliest / timescale;
    for (let i = 0; i < count; i += 1) {
      const reference = view.getUint32(cursor);
      const duration = view.getUint32(cursor + 4) / timescale;
      if (reference >>> 31 || !duration) return null;
      if (seconds >= time && seconds < time + duration) return byte;
      byte += reference & 0x7fffffff;
      time += duration;
      cursor += 12;
    }
    return null;
  }

  button.addEventListener('click', async () => {
    button.disabled = true;
    output.textContent = '';
    const labels = new Map();
    const requests = new Map();
    const flags = { aOpened: false, bFailed: false, aCanceled: false, cRange: false };
    const prefix = new Uint8Array(64 * 1024);
    let prefixLength = 0;
    let expectedByte = null;
    let rollbackTime = 0;
    let releaseA = null;
    let enteredA = null;
    const aEntered = new Promise((resolve) => { enteredA = resolve; });
    let phase = 'setup';
    try {
      window.__p7ProofInvoke = async function (command, args, native) {
        if (command === 'media_resolve') {
          try {
            const result = await native();
            if (args.input === A_BV || args.input === C_BV) {
              log(`${args.input === A_BV ? 'A' : 'C'} resolved token-address=${typeof result.audioUrl === 'string' && result.audioUrl.startsWith('media-token://')}`);
            }
            if (args.input === C_BV) labels.set(result.audioUrl.slice('media-token://'.length), 'C');
            if (args.input === A_BV) labels.set(result.audioUrl.slice('media-token://'.length), 'A');
            return result;
          } catch (error) {
            if (args.input === B_BV && phase === 'overlap') {
              flags.bFailed = true;
              phase = 'recover';
              log('B resolve failed before A completion');
              setTimeout(() => releaseA?.(), 250);
            }
            throw error;
          }
        }
        if (command === 'media_audio_open') {
          const label = labels.get(args.token) || '?';
          if (phase === 'overlap') log(`native open entered label=${label} range=${args.range === null ? 'initial' : 'indexed'}`);
          const head = await native();
          if (phase === 'overlap') log(`native open returned label=${label} status=${head.status}`);
          requests.set(args.requestId, { label, range: args.range });
          if (label === 'A' && phase === 'overlap' && !flags.aOpened) {
            flags.aOpened = true;
            log(`A native GET opened status=${head.status}; holding completion`);
            enteredA();
            await new Promise((resolve) => { releaseA = resolve; });
            log('A open completion released after B failure');
          }
          if (label === 'C' && phase === 'recover') {
            if (args.range === null) log(`C recovery initial GET status=${head.status}`);
            else {
              const actual = /^bytes=(\d+)-$/.exec(args.range)?.[1];
              flags.cRange = head.status === 206 && expectedByte !== null && Number(actual) === expectedByte;
              log(`C indexed Range byte=${actual ?? '?'} expected=${expectedByte ?? '?'} status=${head.status} match=${flags.cRange}`);
            }
          }
          return head;
        }
        if (command === 'media_audio_read') {
          const bytes = await native();
          const request = requests.get(args.requestId);
          if (phase === 'recover' && request?.label === 'C' && request.range === null && expectedByte === null) {
            const value = bytes instanceof Uint8Array ? bytes : new Uint8Array(bytes);
            const length = Math.min(value.length, prefix.length - prefixLength);
            prefix.set(value.subarray(0, length), prefixLength);
            prefixLength += length;
            expectedByte = indexedOffset(prefix.subarray(0, prefixLength), rollbackTime);
            if (expectedByte !== null) log(`C sidx indexed byte=${expectedByte}; prefix=${prefixLength} bytes`);
          }
          return bytes;
        }
        if (command === 'media_audio_cancel') {
          const request = requests.get(args.requestId);
          const result = await native();
          if (request?.label === 'A') {
            flags.aCanceled = result === true;
            log(`A native cancel=${result}`);
          }
          return result;
        }
        return native();
      };

      submit(C_BV);
      const audio = document.querySelector('#audio-player');
      const title = document.querySelector('#video-title');
      await waitFor(() => title.textContent.includes('热歌') && !audio.paused && audio.currentTime > 1, 'C startup');
      log(`C playing title=${title.textContent.trim()}`);
      const progress = document.querySelector('#progress');
      progress.value = String(Math.round(TARGET_TIME / audio.duration * 1000));
      progress.dispatchEvent(new Event('input', { bubbles: true }));
      progress.dispatchEvent(new Event('change', { bubbles: true }));
      await waitFor(() => !audio.paused && audio.currentTime >= TARGET_TIME - 2 && audio.readyState >= 2, 'C distant seek');
      rollbackTime = audio.currentTime;
      log(`C distant playing time=${rollbackTime.toFixed(1)}s`);

      phase = 'overlap';
      submit(A_BV);
      await Promise.race([aEntered, new Promise((_, reject) => setTimeout(() => reject(new Error('A native open timed out')), 30000))]);
      submit(B_BV);
      await waitFor(() => flags.bFailed, 'B failure');
      await waitFor(() => flags.aCanceled && flags.cRange, 'A cancel and C indexed Range');
      await waitFor(() => !audio.paused && audio.currentTime >= rollbackTime - 0.5 && title.textContent.includes('热歌'), 'C playback recovery');
      const first = audio.currentTime;
      await new Promise((resolve) => setTimeout(resolve, 1200));
      const second = audio.currentTime;
      const pass = second > first + 0.5 && document.querySelector('#play-button').getAttribute('aria-label') === '暂停';
      log(`C restored time=${first.toFixed(1)}s then=${second.toFixed(1)}s title=${title.textContent.trim()} playing=${pass}`);
      log(pass ? 'P7-T5 OVERLAP PASS' : 'P7-T5 OVERLAP FAIL: playback did not advance');
    } catch (error) {
      log(`P7-T5 OVERLAP FAIL: ${error instanceof Error ? error.message : String(error)}`);
    } finally {
      delete window.__p7ProofInvoke;
      button.disabled = false;
    }
  });
})();

export {};
