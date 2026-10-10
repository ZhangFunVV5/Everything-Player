export type Bvid = string;

export type BvInput = string;

export interface ResolveRequest {
  input: BvInput;
}

export interface CollectionEpisode {
  bvid: Bvid;
  title: string;
  duration: number | null;
  coverUrl: string;
}

export interface CollectionSection {
  id: string | number;
  title: string;
  episodes: CollectionEpisode[];
}

export interface VideoCollection {
  id: string | number | null;
  title: string;
  sections: CollectionSection[];
}

export interface ResolvedVideo {
  bvid: Bvid;
  cid: number;
  title: string;
  owner: string;
  pageTitle: string;
  duration: number | null;
  coverUrl: string;
  collection: VideoCollection | null;
  audioUrl: string;
}
