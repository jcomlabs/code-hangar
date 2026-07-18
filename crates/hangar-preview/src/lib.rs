use hangar_core::MarkdownLink;
use pulldown_cmark::{html, CowStr, Event, Options, Parser, Tag, TagEnd};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RenderedMarkdown {
    pub html: String,
    pub headings: Vec<String>,
    pub links: Vec<MarkdownLink>,
}

struct OpenLink {
    metadata_index: usize,
    remote: bool,
}

struct OpenImage {
    target: String,
    remote: bool,
    label: String,
}

pub fn render_markdown_safe(markdown: &str) -> RenderedMarkdown {
    let markdown = strip_html_comments_outside_fences(markdown);
    let options = Options::ENABLE_TABLES
        | Options::ENABLE_FOOTNOTES
        | Options::ENABLE_STRIKETHROUGH
        | Options::ENABLE_TASKLISTS
        | Options::ENABLE_SMART_PUNCTUATION;
    let parser = Parser::new_ext(&markdown, options);
    let mut events = Vec::new();
    let mut headings = Vec::new();
    let mut links = Vec::new();
    let mut heading_text: Option<String> = None;
    let mut open_links: Vec<OpenLink> = Vec::new();
    let mut open_image: Option<OpenImage> = None;

    for event in parser {
        if let Some(image) = open_image.as_mut() {
            match event {
                Event::Text(text) | Event::Code(text) => image.label.push_str(&text),
                Event::SoftBreak | Event::HardBreak => image.label.push(' '),
                Event::End(TagEnd::Image) => {
                    let image = open_image.take().expect("open image");
                    let label = image.label.trim();
                    links.push(MarkdownLink {
                        label: if label.is_empty() {
                            "image".to_string()
                        } else {
                            label.to_string()
                        },
                        target: image.target.clone(),
                        is_remote: image.remote,
                    });
                    let rendered = if image.remote {
                        "<span class=\"preview-blocked-inline\">Remote image blocked</span>"
                            .to_string()
                    } else {
                        format!(
                            "<span class=\"preview-local-image\">Local image: {}</span>",
                            escape_html(&image.target)
                        )
                    };
                    events.push(html_event(rendered));
                }
                _ => {}
            }
            continue;
        }

        match event {
            Event::Start(Tag::Heading { .. }) => {
                heading_text = Some(String::new());
                events.push(event);
            }
            Event::End(TagEnd::Heading(level)) => {
                if let Some(text) = heading_text.take() {
                    let text = text.trim();
                    if !text.is_empty() {
                        headings.push(text.to_string());
                    }
                }
                events.push(Event::End(TagEnd::Heading(level)));
            }
            Event::Start(Tag::Link { dest_url, .. }) => {
                let target = dest_url.to_string();
                let remote = is_remote_target(&target);
                let metadata_index = links.len();
                links.push(MarkdownLink {
                    label: String::new(),
                    target: target.clone(),
                    is_remote: remote,
                });
                open_links.push(OpenLink {
                    metadata_index,
                    remote,
                });
                if remote {
                    events.push(html_event(
                        "<span class=\"preview-remote-link\" title=\"Remote link is inert\">"
                            .to_string(),
                    ));
                } else {
                    events.push(html_event(format!(
                        "<a href=\"#\" data-local-path=\"{}\">",
                        escape_attr(&target)
                    )));
                }
            }
            Event::End(TagEnd::Link) => {
                if let Some(link) = open_links.pop() {
                    events.push(html_event(if link.remote {
                        "</span>".to_string()
                    } else {
                        "</a>".to_string()
                    }));
                }
            }
            Event::Start(Tag::Image { dest_url, .. }) => {
                let target = dest_url.to_string();
                open_image = Some(OpenImage {
                    remote: is_remote_target(&target),
                    target,
                    label: String::new(),
                });
            }
            Event::Html(raw) | Event::InlineHtml(raw) => {
                append_visible_text(&raw, &mut heading_text, &mut open_links, &mut links);
                events.push(Event::Text(raw));
            }
            Event::Text(text) => {
                append_visible_text(&text, &mut heading_text, &mut open_links, &mut links);
                events.push(Event::Text(text));
            }
            Event::Code(code) => {
                append_visible_text(&code, &mut heading_text, &mut open_links, &mut links);
                events.push(Event::Code(code));
            }
            Event::SoftBreak => {
                append_visible_text(" ", &mut heading_text, &mut open_links, &mut links);
                events.push(Event::SoftBreak);
            }
            Event::HardBreak => {
                append_visible_text(" ", &mut heading_text, &mut open_links, &mut links);
                events.push(Event::HardBreak);
            }
            other => events.push(other),
        }
    }

    let mut rendered_html = String::new();
    html::push_html(&mut rendered_html, events.into_iter());

    RenderedMarkdown {
        html: rendered_html,
        headings,
        links,
    }
}

fn html_event(html: String) -> Event<'static> {
    Event::Html(CowStr::Boxed(html.into_boxed_str()))
}

fn append_visible_text(
    text: &str,
    heading_text: &mut Option<String>,
    open_links: &mut [OpenLink],
    links: &mut [MarkdownLink],
) {
    if let Some(heading) = heading_text.as_mut() {
        heading.push_str(text);
    }
    if let Some(link) = open_links.last() {
        links[link.metadata_index].label.push_str(text);
    }
}

fn strip_html_comments_outside_fences(markdown: &str) -> String {
    let mut out = String::with_capacity(markdown.len());
    let mut in_fence = false;
    let mut in_comment = false;

    for line in markdown.split_inclusive('\n') {
        if !in_comment
            && (line.trim_start().starts_with("```") || line.trim_start().starts_with("~~~"))
        {
            in_fence = !in_fence;
            out.push_str(line);
            continue;
        }
        if in_fence {
            out.push_str(line);
            continue;
        }

        let mut rest = line;
        loop {
            if in_comment {
                if let Some(end) = rest.find("-->") {
                    rest = &rest[end + 3..];
                    in_comment = false;
                    continue;
                }
                if rest.ends_with('\n') {
                    out.push('\n');
                }
                break;
            }

            if let Some(start) = rest.find("<!--") {
                out.push_str(&rest[..start]);
                rest = &rest[start + 4..];
                in_comment = true;
                continue;
            }
            out.push_str(rest);
            break;
        }
    }

    out
}

fn is_remote_target(target: &str) -> bool {
    let lower = target.trim().to_ascii_lowercase();
    if lower.starts_with("http://")
        || lower.starts_with("https://")
        || lower.starts_with("//")
        || lower.starts_with("data:")
    {
        return true;
    }
    let colon = lower.find(':');
    let slash = lower.find(['/', '\\']).unwrap_or(usize::MAX);
    colon.is_some_and(|index| index < slash)
}

fn escape_html(input: &str) -> String {
    input
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

fn escape_attr(input: &str) -> String {
    escape_html(input).replace('`', "&#96;")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escapes_scripts() {
        let rendered = render_markdown_safe("# Hi\n<script>alert(1)</script>");
        assert!(!rendered.html.contains("<script>"));
        assert!(rendered.html.contains("&lt;script&gt;"));
    }

    #[test]
    fn blocks_remote_images() {
        let rendered = render_markdown_safe("![x](https://example.invalid/a.png)");
        assert!(!rendered.html.contains("https://example.invalid/a.png"));
        assert!(rendered.html.contains("Remote image blocked"));
        assert_eq!(rendered.links[0].label, "x");
        assert!(rendered.links[0].is_remote);
    }

    #[test]
    fn renders_common_markdown_without_literal_markers() {
        let rendered = render_markdown_safe(
            "> _Read this first._\n\n1. **Open** the project\n2. Review ~~old~~ notes",
        );

        assert!(rendered.html.contains("<blockquote>"));
        assert!(rendered.html.contains("<em>Read this first.</em>"));
        assert!(rendered.html.contains("<ol>"));
        assert!(rendered.html.contains("<strong>Open</strong>"));
        assert!(rendered.html.contains("<del>old</del>"));
        assert!(!rendered.html.contains("_Read this first._"));
    }

    #[test]
    fn renders_pipe_tables() {
        let rendered =
            render_markdown_safe("| Name | Status |\n| --- | --- |\n| README | **clean** |");

        assert!(rendered.html.contains("<table>"));
        assert!(rendered.html.contains("<th>Name</th>"));
        assert!(rendered.html.contains("<td><strong>clean</strong></td>"));
        assert!(!rendered.html.contains("| Name | Status |"));
    }

    #[test]
    fn keeps_local_links_clickable_and_remote_links_inert() {
        let rendered = render_markdown_safe(
            "Read [the guide](docs/guide.md) and [the website](https://example.invalid).",
        );

        assert!(rendered.html.contains("data-local-path=\"docs/guide.md\""));
        assert!(rendered.html.contains("preview-remote-link"));
        assert!(!rendered.html.contains("href=\"https://example.invalid\""));
        assert_eq!(rendered.links.len(), 2);
        assert_eq!(rendered.links[0].label, "the guide");
        assert_eq!(rendered.links[1].label, "the website");
    }

    #[test]
    fn hides_html_comments_without_parsing_their_markdown() {
        let rendered = render_markdown_safe(
            "Before <!-- private note --> after\n<!-- hidden\n![internal](docs/private.png)\n-->\nVisible",
        );

        assert!(rendered.html.contains("Before  after"));
        assert!(rendered.html.contains("Visible"));
        assert!(!rendered.html.contains("private note"));
        assert!(!rendered.html.contains("hidden"));
        assert!(rendered.links.is_empty());
    }

    #[test]
    fn keeps_html_comment_syntax_visible_inside_code_fences() {
        let rendered = render_markdown_safe("```html\n<!-- example -->\n```");

        assert!(rendered.html.contains("&lt;!-- example --&gt;"));
    }
}
