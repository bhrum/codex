use crate::{
    AppSummary, Feed, FeedItem, FeedItemKind, HOME_SCHEMA, HomeDocument, QuickReply, Tip, Welcome,
};
use serde::Deserialize;
use sha2::{Digest, Sha256};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceKind {
    Welcome,
    Tip,
    Announcement,
    Article,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContentSource {
    pub path: String,
    pub kind: SourceKind,
    pub markdown: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompiledContent {
    pub home: HomeDocument,
    pub resources: Vec<(String, String)>,
}

pub struct ContentCompiler;

impl ContentCompiler {
    pub fn compile(
        app: AppSummary,
        sources: impl IntoIterator<Item = ContentSource>,
        quick_replies: Vec<QuickReply>,
    ) -> Result<CompiledContent, String> {
        let mut sources = sources.into_iter().collect::<Vec<_>>();
        sources.sort_by(|left, right| left.path.cmp(&right.path));
        let mut welcome = None;
        let mut tips = Vec::new();
        let mut items = Vec::new();
        let mut resources = Vec::new();
        let mut revision_hasher = Sha256::new();

        for source in sources {
            revision_hasher.update(source.path.as_bytes());
            revision_hasher.update([0]);
            revision_hasher.update(source.markdown.as_bytes());
            let (meta, body) = parse_front_matter(&source.markdown)?;
            if meta.id.trim().is_empty() || meta.revision.trim().is_empty() {
                return Err(format!("{} requires stable id and revision", source.path));
            }
            match source.kind {
                SourceKind::Welcome => {
                    if welcome.is_some() {
                        return Err("content may contain only one welcome document".into());
                    }
                    welcome = Some(Welcome {
                        id: meta.id,
                        markdown: body,
                    });
                }
                SourceKind::Tip => tips.push(Tip {
                    id: meta.id,
                    markdown: body,
                    revision: Some(meta.revision),
                }),
                SourceKind::Announcement | SourceKind::Article => {
                    let title = meta
                        .title
                        .filter(|title| !title.trim().is_empty())
                        .ok_or_else(|| format!("{} requires title", source.path))?;
                    let published_at =
                        meta.published_at
                            .filter(|value| !value.trim().is_empty())
                            .ok_or_else(|| format!("{} requires publishedAt", source.path))?;
                    let uri = format!(
                        "mahayana://{}/content/{}/{}",
                        app.id,
                        match source.kind {
                            SourceKind::Announcement => "announcements",
                            _ => "articles",
                        },
                        meta.id
                    );
                    resources.push((uri.clone(), body));
                    items.push(FeedItem {
                        id: meta.id,
                        revision: meta.revision,
                        kind: match source.kind {
                            SourceKind::Announcement => FeedItemKind::Announcement,
                            _ => FeedItemKind::Article,
                        },
                        title,
                        published_at,
                        resource_uri: uri,
                        summary: meta.summary,
                        expires_at: meta.expires_at,
                        cover_image: meta.cover_image,
                        tags: meta.tags,
                        quick_replies: meta.quick_replies,
                    });
                }
            }
        }
        items.sort_by(|left, right| {
            right
                .published_at
                .cmp(&left.published_at)
                .then_with(|| left.id.cmp(&right.id))
        });
        let next_cursor = (items.len() > crate::MAX_HOME_FEED_ITEMS)
            .then(|| crate::MAX_HOME_FEED_ITEMS.to_string());
        items.truncate(crate::MAX_HOME_FEED_ITEMS);
        let revision = format!("{:x}", revision_hasher.finalize());
        let home = HomeDocument {
            schema: HOME_SCHEMA.into(),
            revision,
            app,
            welcome,
            tips,
            quick_replies,
            feed: Feed { items, next_cursor },
        };
        home.validate().map_err(|error| error.to_string())?;
        Ok(CompiledContent { home, resources })
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct FrontMatter {
    id: String,
    revision: String,
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    published_at: Option<String>,
    #[serde(default)]
    summary: Option<String>,
    #[serde(default)]
    expires_at: Option<String>,
    #[serde(default)]
    cover_image: Option<String>,
    #[serde(default)]
    tags: Vec<String>,
    #[serde(default)]
    quick_replies: Vec<QuickReply>,
}

fn parse_front_matter(source: &str) -> Result<(FrontMatter, String), String> {
    let normalized = source.replace("\r\n", "\n");
    let rest = normalized
        .strip_prefix("---\n")
        .ok_or_else(|| "Markdown content requires YAML front matter".to_string())?;
    let (front_matter, body) = rest
        .split_once("\n---\n")
        .ok_or_else(|| "Markdown front matter is not closed".to_string())?;
    let metadata = serde_yaml::from_str(front_matter)
        .map_err(|error| format!("invalid Markdown front matter: {error}"))?;
    Ok((metadata, body.trim().to_string()))
}
