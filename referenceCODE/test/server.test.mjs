import assert from 'node:assert/strict';
import test from 'node:test';

import {
  chooseAudioTrack,
  extractBvid,
  extractCollection,
  forwardableAudioHeaders,
  normalizeCoverUrl,
} from '../server.mjs';

test('extractBvid accepts a BV id and a video URL', () => {
  assert.equal(extractBvid('BV1xx411c7mD'), 'BV1xx411c7mD');
  assert.equal(
    extractBvid('https://www.bilibili.com/video/BV1xx411c7mD?p=2'),
    'BV1xx411c7mD',
  );
  assert.equal(extractBvid('not-a-bvid'), null);
});

test('normalizeCoverUrl upgrades Bilibili image URLs to HTTPS', () => {
  assert.equal(normalizeCoverUrl('//i0.hdslb.com/test.jpg'), 'https://i0.hdslb.com/test.jpg');
  assert.equal(normalizeCoverUrl('http://i0.hdslb.com/test.jpg'), 'https://i0.hdslb.com/test.jpg');
});

test('chooseAudioTrack selects the highest bandwidth track', () => {
  const selected = chooseAudioTrack({
    dash: { audio: [{ baseUrl: 'low', bandwidth: 64000 }, { baseUrl: 'high', bandwidth: 192000 }] },
  });
  assert.equal(selected.baseUrl, 'high');
});

test('extractCollection exposes cover, title and duration', () => {
  const collection = extractCollection({
    ugc_season: {
      id: 42,
      title: '测试合集',
      sections: [
        {
          title: '正片',
          episodes: [
            {
              bvid: 'BV1xx411c7mD',
              title: '第一集',
              page: { duration: 125 },
              arc: { pic: 'http://i0.hdslb.com/cover.jpg' },
            },
          ],
        },
      ],
    },
  });
  assert.equal(collection.title, '测试合集');
  assert.equal(collection.sections[0].episodes[0].coverUrl, 'https://i0.hdslb.com/cover.jpg');
});

test('forwardableAudioHeaders keeps range metadata', () => {
  const headers = new Headers({
    'content-type': 'audio/mp4',
    'content-range': 'bytes 0-1023/5000',
    'content-length': '1024',
    server: 'example',
  });
  assert.deepEqual(forwardableAudioHeaders(headers), {
    'content-length': '1024',
    'content-range': 'bytes 0-1023/5000',
    'content-type': 'audio/mp4',
  });
});
