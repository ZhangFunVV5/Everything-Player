use serde::Serialize;
use serde_json::{Map, Value};
use std::cmp::Ordering;

/// The collection DTOs intentionally mirror `src/types/media.ts`.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CollectionEpisode {
    pub bvid: String,
    pub title: String,
    pub duration: Option<Value>,
    pub cover_url: String,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CollectionSection {
    pub id: Value,
    pub title: String,
    pub episodes: Vec<CollectionEpisode>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VideoCollection {
    pub id: Option<Value>,
    pub title: String,
    pub sections: Vec<CollectionSection>,
}

/// Metadata selected from a view response. `cid: None` is deliberately
/// representable so the later native service can report a missing CID without
/// putting HTTP or command concerns in this module.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SelectedVideoMetadata {
    pub bvid: String,
    pub cid: Option<Value>,
    pub title: String,
    pub owner: String,
    pub page_title: String,
    pub duration: Option<Value>,
    pub cover_url: String,
    pub collection: Option<VideoCollection>,
}

impl SelectedVideoMetadata {
    /// Matches the legacy resolve gate: a JSON null/missing/falsey CID is not
    /// usable by the later play-url request.
    pub fn has_usable_cid(&self) -> bool {
        self.cid.as_ref().is_some_and(is_legacy_truthy)
    }
}

/// Internal-only result of DASH selection. It intentionally does not derive
/// `Serialize`: CDN URLs stay inside native state and cannot be exposed by a
/// DTO serialization in this task.
#[derive(Debug, Clone, PartialEq)]
pub struct SelectedAudioTrack {
    pub primary_url: String,
    pub backup_urls: Vec<String>,
    pub bandwidth: Option<Value>,
}

/// Extract a BV value using the exact boundary shape of the legacy regex.
pub fn extract_bvid(input: &str) -> Option<String> {
    let input = input.trim_matches(is_legacy_trim_whitespace);
    let bytes = input.as_bytes();
    if bytes.len() < 12 {
        return None;
    }

    for start in 0..=bytes.len() - 12 {
        if start != 0 && bytes[start - 1] != b'/' {
            continue;
        }
        if !bytes[start].eq_ignore_ascii_case(&b'B')
            || !bytes[start + 1].eq_ignore_ascii_case(&b'V')
            || !bytes[start + 2..start + 12]
                .iter()
                .all(u8::is_ascii_alphanumeric)
        {
            continue;
        }

        if !matches!(
            bytes.get(start + 12),
            None | Some(b'/') | Some(b'?') | Some(b'#')
        ) {
            continue;
        }

        let suffix = &input[start + 2..start + 12];
        return Some(format!("BV{suffix}"));
    }
    None
}

/// JSON-facing convenience wrapper for the legacy non-string-input result.
pub fn extract_bvid_value(input: &Value) -> Option<String> {
    input.as_str().and_then(extract_bvid)
}

pub fn normalize_cover_url(url: Option<&str>) -> String {
    let Some(url) = url else {
        return String::new();
    };
    if url.trim_matches(is_legacy_trim_whitespace).is_empty() {
        return String::new();
    }
    if url.starts_with("//") {
        return format!("https:{url}");
    }
    if url
        .as_bytes()
        .get(..7)
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case(b"http://"))
    {
        return format!("https://{}", &url[7..]);
    }
    url.to_owned()
}

pub fn select_video_metadata(video_data: &Value, extracted_bvid: &str) -> SelectedVideoMetadata {
    let data = video_data.as_object();
    let page = data
        .and_then(|data| data.get("pages"))
        .and_then(Value::as_array)
        .and_then(|pages| pages.first())
        .and_then(Value::as_object);

    let cid = page
        .and_then(|page| nullish_value(page.get("cid")))
        .or_else(|| data.and_then(|data| nullish_value(data.get("cid"))));
    let duration = page
        .and_then(|page| nullish_value(page.get("duration")))
        .or_else(|| data.and_then(|data| nullish_value(data.get("duration"))));

    let bvid = data
        .and_then(|data| truthy_string(data.get("bvid")))
        .unwrap_or_else(|| extracted_bvid.to_owned());
    let title = data
        .and_then(|data| truthy_string(data.get("title")))
        .unwrap_or_else(|| extracted_bvid.to_owned());
    let owner = data
        .and_then(|data| data.get("owner"))
        .and_then(Value::as_object)
        .and_then(|owner| truthy_string(owner.get("name")))
        .unwrap_or_default();
    let page_title = page
        .and_then(|page| truthy_string(page.get("part")))
        .unwrap_or_default();
    let cover_url = data
        .and_then(|data| truthy_string(data.get("pic")))
        .map(|pic| normalize_cover_url(Some(&pic)))
        .unwrap_or_default();

    SelectedVideoMetadata {
        bvid,
        cid,
        title,
        owner,
        page_title,
        duration,
        cover_url,
        collection: extract_collection(video_data),
    }
}

pub fn extract_collection(video_data: &Value) -> Option<VideoCollection> {
    let season = video_data.get("ugc_season").and_then(Value::as_object)?;
    let raw_sections = season.get("sections").and_then(Value::as_array)?;

    let sections = raw_sections
        .iter()
        .enumerate()
        .filter_map(|(section_index, raw_section)| {
            let section = raw_section.as_object();
            let raw_episodes = section
                .and_then(|section| section.get("episodes"))
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();

            let episodes = raw_episodes
                .iter()
                .filter_map(extract_collection_episode)
                .collect::<Vec<_>>();
            if episodes.is_empty() {
                return None;
            }

            let id = section
                .and_then(|section| nullish_value(section.get("id")))
                .or_else(|| section.and_then(|section| nullish_value(section.get("section_id"))))
                .unwrap_or_else(|| Value::from(section_index));
            let title = section
                .and_then(|section| truthy_string(section.get("title")))
                .unwrap_or_default();

            Some(CollectionSection {
                id,
                title,
                episodes,
            })
        })
        .collect::<Vec<_>>();

    if sections.is_empty() {
        return None;
    }

    let id = nullish_value(season.get("id")).or_else(|| nullish_value(season.get("season_id")));
    let title = truthy_string(season.get("title")).unwrap_or_else(|| "视频合集".to_owned());
    Some(VideoCollection {
        id,
        title,
        sections,
    })
}

pub fn choose_audio_track(play_data: &Value) -> Option<SelectedAudioTrack> {
    let dash = play_data.get("dash").and_then(Value::as_object)?;
    let mut candidates = Vec::new();

    append_audio_array(&mut candidates, dash.get("audio"));
    let dolby = dash.get("dolby").and_then(Value::as_object);
    append_audio_array(&mut candidates, dolby.and_then(|dolby| dolby.get("audio")));
    if let Some(audio) = dash
        .get("flac")
        .and_then(Value::as_object)
        .and_then(|flac| flac.get("audio"))
        .filter(|audio| is_legacy_truthy(audio))
    {
        candidates.push(audio);
    }

    let mut selected = None;
    for candidate in candidates
        .into_iter()
        .filter_map(SelectedAudioTrack::from_value)
    {
        if selected.as_ref().map_or(true, |current| {
            compare_bandwidth(&candidate, current) == Ordering::Greater
        }) {
            selected = Some(candidate);
        }
    }
    selected
}

fn append_audio_array<'a>(candidates: &mut Vec<&'a Value>, value: Option<&'a Value>) {
    if let Some(audio) = value.and_then(Value::as_array) {
        candidates.extend(audio.iter());
    }
}

impl SelectedAudioTrack {
    fn from_value(track: &Value) -> Option<Self> {
        let object = track.as_object()?;
        let primary_url = first_truthy_string(object, &["baseUrl", "base_url"])?;
        let backup_urls = backup_urls(object);
        let bandwidth = nullish_value(object.get("bandwidth"));
        Some(Self {
            primary_url,
            backup_urls,
            bandwidth,
        })
    }
}

fn compare_bandwidth(left: &SelectedAudioTrack, right: &SelectedAudioTrack) -> Ordering {
    bandwidth_score(left)
        .partial_cmp(&bandwidth_score(right))
        .unwrap_or(Ordering::Equal)
}

fn bandwidth_score(track: &SelectedAudioTrack) -> f64 {
    track
        .bandwidth
        .as_ref()
        .and_then(|value| match value {
            Value::Number(number) => number.as_f64(),
            Value::String(string) => string.parse::<f64>().ok(),
            _ => None,
        })
        .unwrap_or(0.0)
}

fn backup_urls(object: &Map<String, Value>) -> Vec<String> {
    let backup = object
        .get("backupUrl")
        .filter(|value| is_legacy_truthy(value))
        .or_else(|| {
            object
                .get("backup_url")
                .filter(|value| is_legacy_truthy(value))
        });
    backup
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .filter(|url| !url.is_empty())
        .map(str::to_owned)
        .collect()
}

fn extract_collection_episode(value: &Value) -> Option<CollectionEpisode> {
    let episode = value.as_object()?;
    let arc = episode.get("arc").and_then(Value::as_object);
    let bvid = first_truthy_string(episode, &["bvid"])
        .or_else(|| arc.and_then(|arc| truthy_string(arc.get("bvid"))))?;
    let title = first_truthy_string(episode, &["title"])
        .or_else(|| arc.and_then(|arc| truthy_string(arc.get("title"))))
        .or_else(|| truthy_string(episode.get("bvid")))
        .unwrap_or_default();
    let duration = episode
        .get("page")
        .and_then(Value::as_object)
        .and_then(|page| nullish_value(page.get("duration")))
        .or_else(|| arc.and_then(|arc| nullish_value(arc.get("duration"))));
    let cover_url = arc
        .and_then(|arc| truthy_string(arc.get("pic")))
        .or_else(|| truthy_string(episode.get("cover")))
        .map(|cover| normalize_cover_url(Some(&cover)))
        .unwrap_or_default();

    Some(CollectionEpisode {
        bvid,
        title,
        duration,
        cover_url,
    })
}

fn first_truthy_string(object: &Map<String, Value>, names: &[&str]) -> Option<String> {
    names
        .iter()
        .find_map(|name| truthy_string(object.get(*name)))
}

fn truthy_string(value: Option<&Value>) -> Option<String> {
    value
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
}

fn nullish_value(value: Option<&Value>) -> Option<Value> {
    value.filter(|value| !value.is_null()).cloned()
}

fn is_legacy_truthy(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(value) => *value,
        Value::Number(value) => value.as_f64().is_some_and(|value| value != 0.0),
        Value::String(value) => !value.is_empty(),
        Value::Array(_) | Value::Object(_) => true,
    }
}

fn is_legacy_trim_whitespace(value: char) -> bool {
    matches!(
        value,
        '\u{0009}'..='\u{000d}'
            | '\u{0020}'
            | '\u{00a0}'
            | '\u{1680}'
            | '\u{2000}'..='\u{200a}'
            | '\u{2028}'
            | '\u{2029}'
            | '\u{202f}'
            | '\u{205f}'
            | '\u{3000}'
            | '\u{feff}'
    )
}

#[cfg(test)]
mod bilibili_tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn bilibili_extract_bvid_matches_legacy_boundaries_and_case() {
        assert_eq!(extract_bvid("BV1xx411c7mD"), Some("BV1xx411c7mD".into()));
        assert_eq!(
            extract_bvid("  https://www.bilibili.com/video/bv1xx411c7mD?p=2#part  \n"),
            Some("BV1xx411c7mD".into())
        );
        assert_eq!(
            extract_bvid("/BV1xx411c7mD/part"),
            Some("BV1xx411c7mD".into())
        );
        assert_eq!(
            extract_bvid("/BV1xx411c7mD?x=1"),
            Some("BV1xx411c7mD".into())
        );
        assert_eq!(
            extract_bvid("/BV1xx411c7mD#page"),
            Some("BV1xx411c7mD".into())
        );
        assert_eq!(extract_bvid("/bV1xx411c7mD"), Some("BV1xx411c7mD".into()));
        assert_eq!(extract_bvid("prefixBV1xx411c7mD"), None);
        assert_eq!(extract_bvid("/BV1xx411c7mD0"), None);
        assert_eq!(extract_bvid("/BV1xx411c7mD."), None);
        assert_eq!(extract_bvid("/BV1xx411c7m"), None);
        assert_eq!(extract_bvid_value(&json!(123)), None);
    }

    #[test]
    fn bilibili_normalize_cover_url_matches_protocol_rules() {
        assert_eq!(
            normalize_cover_url(Some("//i0.hdslb.com/test.jpg")),
            "https://i0.hdslb.com/test.jpg"
        );
        assert_eq!(
            normalize_cover_url(Some("HTTP://i0.hdslb.com/test.jpg")),
            "https://i0.hdslb.com/test.jpg"
        );
        assert_eq!(
            normalize_cover_url(Some("https://i0.hdslb.com/test.jpg")),
            "https://i0.hdslb.com/test.jpg"
        );
        assert_eq!(normalize_cover_url(None), "");
        assert_eq!(normalize_cover_url(Some(" \t\n")), "");
        assert_eq!(
            normalize_cover_url(Some("cdn.example/cover.jpg")),
            "cdn.example/cover.jpg"
        );
        assert_eq!(
            normalize_cover_url(Some(" http://example/cover.jpg")),
            " http://example/cover.jpg"
        );
    }

    #[test]
    fn bilibili_first_page_selection_and_metadata_fallbacks_match_legacy() {
        let view = json!({
            "bvid": "",
            "title": "",
            "owner": {},
            "pic": "http://example.com/video.jpg",
            "duration": 999,
            "cid": 9099,
            "pages": [
                { "cid": 9101, "part": "First page", "duration": 111 },
                { "cid": 9102, "part": "Second page", "duration": 222 }
            ]
        });
        let selected = select_video_metadata(&view, "BV1A2B3C4D5E");
        assert_eq!(selected.bvid, "BV1A2B3C4D5E");
        assert_eq!(selected.cid, Some(json!(9101)));
        assert_eq!(selected.title, "BV1A2B3C4D5E");
        assert_eq!(selected.owner, "");
        assert_eq!(selected.page_title, "First page");
        assert_eq!(selected.duration, Some(json!(111)));
        assert_eq!(selected.cover_url, "https://example.com/video.jpg");
        assert!(selected.has_usable_cid());

        let fallback = json!({
            "bvid": "BV1F2G3H4J5K",
            "title": "Title",
            "owner": { "name": "UP" },
            "duration": 42,
            "cid": 9099,
            "pages": [{ "cid": null, "part": "", "duration": null }]
        });
        let selected = select_video_metadata(&fallback, "BV1A2B3C4D5E");
        assert_eq!(selected.cid, Some(json!(9099)));
        assert_eq!(selected.duration, Some(json!(42)));
        assert!(selected.has_usable_cid());

        let no_cid = select_video_metadata(&json!({ "pages": [], "cid": null }), "BV1A2B3C4D5E");
        assert_eq!(no_cid.cid, None);
        assert!(!no_cid.has_usable_cid());
    }

    #[test]
    fn bilibili_collection_preserves_order_fallbacks_and_cleans_empty_entries() {
        let video = json!({
            "ugc_season": {
                "season_id": "season-7",
                "title": "",
                "sections": [
                    {
                        "id": null,
                        "section_id": "opening",
                        "title": "Opening",
                        "episodes": [
                            {
                                "bvid": "BV1DIRECT000",
                                "title": "Direct title",
                                "page": { "duration": 31 },
                                "arc": { "pic": "http://example.com/direct.jpg" }
                            },
                            {
                                "arc": {
                                    "bvid": "BV1ARC00000",
                                    "title": "Arc title",
                                    "duration": 32,
                                    "pic": "//example.com/arc.jpg"
                                },
                                "cover": "http://example.com/fallback.jpg"
                            },
                            { "title": "missing bvid" }
                        ]
                    },
                    { "title": "empty section", "episodes": [] },
                    {
                        "title": "Indexed",
                        "episodes": [{ "bvid": "BV1INDEX000", "cover": "HTTP://example.com/index.jpg" }]
                    }
                ]
            }
        });
        let collection = extract_collection(&video).expect("collection");
        assert_eq!(collection.id, Some(json!("season-7")));
        assert_eq!(collection.title, "视频合集");
        assert_eq!(collection.sections.len(), 2);
        assert_eq!(collection.sections[0].id, json!("opening"));
        assert_eq!(collection.sections[1].id, json!(2));
        assert_eq!(collection.sections[0].episodes[0].bvid, "BV1DIRECT000");
        assert_eq!(collection.sections[0].episodes[0].duration, Some(json!(31)));
        assert_eq!(
            collection.sections[0].episodes[0].cover_url,
            "https://example.com/direct.jpg"
        );
        assert_eq!(collection.sections[0].episodes[1].title, "Arc title");
        assert_eq!(collection.sections[0].episodes[1].duration, Some(json!(32)));
        assert_eq!(
            collection.sections[0].episodes[1].cover_url,
            "https://example.com/arc.jpg"
        );
        assert_eq!(collection.sections[1].episodes[0].duration, None);
        assert_eq!(
            collection.sections[1].episodes[0].cover_url,
            "https://example.com/index.jpg"
        );

        let serialized = serde_json::to_value(&collection).expect("collection serialization");
        assert_eq!(
            serialized["sections"][0]["episodes"][0]["coverUrl"],
            json!("https://example.com/direct.jpg")
        );
        assert_eq!(
            serialized["sections"][0]["episodes"][1]["duration"],
            json!(32)
        );
        assert_eq!(
            extract_collection(&json!({ "ugc_season": { "sections": [{ "episodes": [] }] } })),
            None
        );
        assert_eq!(extract_collection(&json!({})), None);
    }

    #[test]
    fn bilibili_audio_selection_uses_all_dash_sources_and_preserves_urls() {
        let selected = choose_audio_track(&json!({
            "dash": {
                "audio": [
                    { "baseUrl": "ordinary", "backupUrl": ["ordinary-backup"], "bandwidth": "192000" },
                    { "backupUrl": ["ignored"], "bandwidth": 999999 }
                ],
                "dolby": { "audio": [{ "base_url": "dolby", "backup_url": ["dolby-backup"], "bandwidth": 256000 }] },
                "flac": { "audio": { "base_url": "flac", "backup_url": ["flac-backup-1", "flac-backup-2"], "bandwidth": 320000 } }
            }
        })).expect("selected track");
        assert_eq!(selected.primary_url, "flac");
        assert_eq!(selected.backup_urls, vec!["flac-backup-1", "flac-backup-2"]);
        assert_eq!(selected.bandwidth, Some(json!(320000)));
    }

    #[test]
    fn bilibili_audio_selection_is_stable_for_equal_or_missing_bandwidth() {
        let selected = choose_audio_track(&json!({
            "dash": {
                "audio": [
                    { "base_url": "first", "bandwidth": "100" },
                    { "baseUrl": "second", "bandwidth": 100 },
                    { "baseUrl": "zero", "bandwidth": 0 },
                    { "baseUrl": "missing" }
                ],
                "dolby": { "audio": [{ "baseUrl": "third", "bandwidth": "100" }] }
            }
        }))
        .expect("selected track");
        assert_eq!(selected.primary_url, "first");

        assert_eq!(
            choose_audio_track(&json!({ "dash": { "audio": [{ "bandwidth": 100 }] } })),
            None
        );
        assert_eq!(
            choose_audio_track(
                &json!({ "dash": { "audio": [], "dolby": { "audio": [] }, "flac": { "audio": null } } })
            ),
            None
        );
    }
}
