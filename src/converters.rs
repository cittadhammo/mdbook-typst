use std::collections::{HashSet, VecDeque};
use std::marker::PhantomData;
use std::path::{Path, PathBuf};

use pullup::converter;
use pullup::markdown::{CodeBlockKind, CowStr, Event as MdEvent, Tag as MdTag};
use pullup::mdbook::{Event as MdbookEvent, Tag as MdbookTag};
use pullup::typst::{CodeBlockDisplay, Event as TypstEvent, Tag as TypstTag};
use pullup::ParserEvent;

/// Convert mdBook parts to chapters with cover pages.
#[derive(Debug)]
pub struct PartToCoverPage<'a, T> {
    in_part: bool,
    iter: T,
    _p: PhantomData<&'a ()>,
}

impl<'a, T> PartToCoverPage<'a, T>
where
    T: Iterator<Item = ParserEvent<'a>>,
{
    #[allow(dead_code)]
    pub fn new(iter: T) -> Self {
        Self {
            in_part: false,
            iter,
            _p: PhantomData,
        }
    }
}

impl<'a, T> Iterator for PartToCoverPage<'a, T>
where
    T: Iterator<Item = ParserEvent<'a>>,
{
    type Item = ParserEvent<'a>;

    fn next(&mut self) -> Option<Self::Item> {
        match (self.in_part, self.iter.next()) {
            (_, Some(ParserEvent::Mdbook(MdbookEvent::Start(MdbookTag::Part(name, _))))) => {
                if let Some(name) = name {
                    self.in_part = true;
                    Some(ParserEvent::Typst(TypstEvent::Raw(
                        format!(
                            r#"
                        #set page(
                            header: none,
                        )
                        #heading(level: 1, outlined: true, "{}")
                        #pagebreak()
                        #set page(
                            header:  text(size: 8pt, fill: gray)[#align(right)[{}]],
                        )
                        {}"#,
                            name, name, '\n'
                        )
                        .into(),
                    )))
                } else {
                    self.in_part = false;
                    self.next()
                }
            }
            (_, Some(ParserEvent::Mdbook(MdbookEvent::End(MdbookTag::Part(_, _))))) => {
                self.in_part = false;
                Some(ParserEvent::Typst(TypstEvent::FunctionCall(
                    None,
                    "pagebreak".into(),
                    vec!["weak: true".into()],
                )))
            }
            (
                true,
                Some(ParserEvent::Typst(TypstEvent::Start(TypstTag::Heading(num, toc, bookmarks, label)))),
            ) => Some(ParserEvent::Typst(TypstEvent::Start(TypstTag::Heading(
                num.saturating_add(1),
                toc,
                bookmarks,
                label,
            )))),
            (
                true,
                Some(ParserEvent::Typst(TypstEvent::End(TypstTag::Heading(num, toc, bookmarks, label)))),
            ) => Some(ParserEvent::Typst(TypstEvent::End(TypstTag::Heading(
                num.saturating_add(1),
                toc,
                bookmarks,
                label,
            )))),
            (_, x) => x,
        }
    }
}

converter!(
    /// Convert parts to cover pages.
    PartToCoverPageX,
    ParserEvent<'a> => ParserEvent<'a>,
    |this: &mut Self| {
        match this.iter.next() {
            Some(ParserEvent::Mdbook(MdbookEvent::Start(MdbookTag::Part(name, _)))) => {
                if let Some(name) = name {
                    Some(ParserEvent::Typst(TypstEvent::Raw(
                        format!(
                        r#"
                        #set page(
                            header: none,
                        )
                        #heading(level: 1, outlined: false, "{}")
                        #pagebreak()
                        #set page(
                            header:  text(size: 8pt, fill: gray)[#align(right)[{}]],
                        )
                        {}"#, name, name, '\n').into()))
                    )
                } else {
                    this.next()
                }
            },
            Some(ParserEvent::Mdbook(MdbookEvent::End(MdbookTag::Part( _, _)))) => {
                Some(ParserEvent::Typst(TypstEvent::FunctionCall(
                    None,
                    "pagebreak".into(),
                    vec!["weak: true".into()],
                )))
            },
            x => x,
    }
});

/// Parse mdBook code block info string into language and attributes.
/// Format: "lang" or "lang,attr1,attr2" or "lang,attr1,key=value"
/// Examples:
///   - "rust" -> ("rust", None)
///   - "rust,ignore" -> ("rust", None)
///   - "rust,editable" -> ("rust", None)
///   - "rust,ignore,filename=\"src/main.rs\"" -> ("rust", Some("src/main.rs"))
fn parse_code_info(info: &str) -> (Option<String>, Option<String>) {
    if info.is_empty() {
        return (None, None);
    }

    let mut parts = info.split(',');

    // First part is the language
    let lang = parts.next().and_then(|s| {
        let trimmed = s.trim();
        if trimmed.is_empty() {
            None
        } else {
            Some(trimmed.to_string())
        }
    });

    // Look for filename attribute in remaining parts
    let mut filename = None;
    for part in parts {
        let part = part.trim();
        // Handle both filename="..." and filename=...
        if let Some(rest) = part.strip_prefix("filename=") {
            // Remove surrounding quotes if present
            let value = rest.trim_matches('"').trim_matches('\'');
            filename = Some(value.to_string());
        }
    }

    (lang, filename)
}

/// Fix mdBook code block fence names.
///
/// mdBook uses comma-separated info strings like `rust,ignore` or
/// `rust,editable,filename="src/main.rs"`. This converter:
/// 1. Extracts the language (first comma-separated value)
/// 2. Extracts the filename if present and displays it above the code block
/// 3. Ignores other mdBook-specific attributes (ignore, editable, etc.)
#[derive(Debug)]
pub struct FixCodeBlockFence<'a, T> {
    buf: VecDeque<ParserEvent<'a>>,
    iter: T,
}

impl<'a, T> FixCodeBlockFence<'a, T>
where
    T: Iterator<Item = ParserEvent<'a>>,
{
    pub fn new(iter: T) -> Self {
        Self {
            buf: VecDeque::new(),
            iter,
        }
    }
}

impl<'a, T> Iterator for FixCodeBlockFence<'a, T>
where
    T: Iterator<Item = ParserEvent<'a>>,
{
    type Item = ParserEvent<'a>;

    fn next(&mut self) -> Option<Self::Item> {
        // Return buffered events first
        if let Some(event) = self.buf.pop_front() {
            return Some(event);
        }

        match self.iter.next() {
            // Handle Typst code blocks (already converted)
            Some(ParserEvent::Typst(TypstEvent::Start(TypstTag::CodeBlock(
                Some(ref fence),
                display,
            )))) => {
                let (lang, filename) = parse_code_info(fence.as_ref());
                let lang_cow: Option<CowStr<'a>> = lang.map(CowStr::from);

                // If there's a filename, emit it as a styled block before the code
                if let Some(ref fname) = filename {
                    self.buf.push_back(ParserEvent::Typst(TypstEvent::Start(
                        TypstTag::CodeBlock(lang_cow, display),
                    )));
                    // Return the filename display first
                    Some(ParserEvent::Typst(TypstEvent::Raw(
                        format!(
                            "#block(width: 100%, inset: (x: 0.5em, y: 0.3em), fill: luma(240), radius: (top: 4pt))[#text(size: 0.85em, style: \"italic\")[{}]]\n",
                            fname
                        )
                        .into(),
                    )))
                } else {
                    // Just emit the code block with corrected language
                    Some(ParserEvent::Typst(TypstEvent::Start(TypstTag::CodeBlock(
                        lang_cow, display,
                    ))))
                }
            }

            // Handle Markdown code blocks (not yet converted)
            Some(ParserEvent::Markdown(MdEvent::Start(MdTag::CodeBlock(
                CodeBlockKind::Fenced(ref fence),
            )))) => {
                let (lang, filename) = parse_code_info(fence.as_ref());
                let lang_cow: Option<CowStr<'a>> = lang.map(CowStr::from);

                // If there's a filename, emit it as a styled block before the code
                if let Some(ref fname) = filename {
                    self.buf.push_back(ParserEvent::Typst(TypstEvent::Start(
                        TypstTag::CodeBlock(lang_cow, CodeBlockDisplay::Block),
                    )));
                    // Return the filename display first
                    Some(ParserEvent::Typst(TypstEvent::Raw(
                        format!(
                            "#block(width: 100%, inset: (x: 0.5em, y: 0.3em), fill: luma(240), radius: (top: 4pt))[#text(size: 0.85em, style: \"italic\")[{}]]\n",
                            fname
                        )
                        .into(),
                    )))
                } else {
                    // Just emit the code block with corrected language
                    Some(ParserEvent::Typst(TypstEvent::Start(TypstTag::CodeBlock(
                        lang_cow,
                        CodeBlockDisplay::Block,
                    ))))
                }
            }

            // Handle mdBook wrapped markdown code blocks
            Some(ParserEvent::Mdbook(MdbookEvent::MarkdownContentEvent(MdEvent::Start(
                MdTag::CodeBlock(CodeBlockKind::Fenced(ref fence)),
            )))) => {
                let (lang, filename) = parse_code_info(fence.as_ref());
                let lang_cow: Option<CowStr<'a>> = lang.map(CowStr::from);

                // If there's a filename, emit it as a styled block before the code
                if let Some(ref fname) = filename {
                    self.buf.push_back(ParserEvent::Typst(TypstEvent::Start(
                        TypstTag::CodeBlock(lang_cow, CodeBlockDisplay::Block),
                    )));
                    // Return the filename display first
                    Some(ParserEvent::Typst(TypstEvent::Raw(
                        format!(
                            "#block(width: 100%, inset: (x: 0.5em, y: 0.3em), fill: luma(240), radius: (top: 4pt))[#text(size: 0.85em, style: \"italic\")[{}]]\n",
                            fname
                        )
                        .into(),
                    )))
                } else {
                    // Just emit the code block with corrected language
                    Some(ParserEvent::Typst(TypstEvent::Start(TypstTag::CodeBlock(
                        lang_cow,
                        CodeBlockDisplay::Block,
                    ))))
                }
            }

            x => x,
        }
    }
}

/// Convert blockquotes that start with "Note:", "Warning:", etc. to styled callouts.
///
/// mdBook uses blockquotes with special prefixes for notes and warnings:
/// - `> Note: ...` -> informational note
/// - `> Warning: ...` -> warning callout
#[derive(Debug)]
pub struct ConvertCallouts<'a, T> {
    in_blockquote: bool,
    blockquote_events: Vec<ParserEvent<'a>>,
    output_buffer: VecDeque<ParserEvent<'a>>,
    iter: T,
}

impl<'a, T> ConvertCallouts<'a, T>
where
    T: Iterator<Item = ParserEvent<'a>>,
{
    pub fn new(iter: T) -> Self {
        Self {
            in_blockquote: false,
            blockquote_events: Vec::new(),
            output_buffer: VecDeque::new(),
            iter,
        }
    }
}

/// Detect callout type from text content.
fn detect_callout_type(text: &str) -> Option<(&str, &str, &str)> {
    let text = text.trim_start();

    // Check for various callout prefixes
    let patterns = [
        ("Note:", "note", "rgb(\"#1976D2\")"),        // Blue
        ("Warning:", "warning", "rgb(\"#F57C00\")"), // Orange
        ("Tip:", "tip", "rgb(\"#388E3C\")"),         // Green
        ("Important:", "important", "rgb(\"#D32F2F\")"), // Red
        ("Caution:", "caution", "rgb(\"#F57C00\")"), // Orange
        ("Info:", "info", "rgb(\"#1976D2\")"),       // Blue
    ];

    for (prefix, callout_type, color) in patterns {
        if text.starts_with(prefix) {
            return Some((prefix, callout_type, color));
        }
    }

    None
}

impl<'a, T> Iterator for ConvertCallouts<'a, T>
where
    T: Iterator<Item = ParserEvent<'a>>,
{
    type Item = ParserEvent<'a>;

    fn next(&mut self) -> Option<Self::Item> {
        // Return buffered events first
        if let Some(event) = self.output_buffer.pop_front() {
            return Some(event);
        }

        match self.iter.next() {
            // Start of a blockquote - buffer events until we can analyze
            Some(ParserEvent::Typst(TypstEvent::Start(TypstTag::Quote(ty, quotes, attr)))) => {
                self.in_blockquote = true;
                self.blockquote_events.clear();
                self.blockquote_events
                    .push(ParserEvent::Typst(TypstEvent::Start(TypstTag::Quote(
                        ty, quotes, attr,
                    ))));
                self.next()
            }

            // End of blockquote - analyze and emit
            Some(ParserEvent::Typst(TypstEvent::End(TypstTag::Quote(ty, quotes, attr))))
                if self.in_blockquote =>
            {
                self.in_blockquote = false;
                self.blockquote_events
                    .push(ParserEvent::Typst(TypstEvent::End(TypstTag::Quote(
                        ty, quotes, attr,
                    ))));

                // Check if the first text event starts with a callout prefix
                let mut callout_info: Option<(String, String, String)> = None;
                for event in &self.blockquote_events {
                    if let ParserEvent::Typst(TypstEvent::Text(text)) = event {
                        if let Some((prefix, callout_type, color)) =
                            detect_callout_type(text.as_ref())
                        {
                            callout_info = Some((
                                prefix.to_string(),
                                callout_type.to_string(),
                                color.to_string(),
                            ));
                        }
                        break;
                    }
                }

                if let Some((prefix, callout_type, color)) = callout_info {
                    // Convert to a styled callout
                    let header = format!(
                        "#block(width: 100%, stroke: (left: 3pt + {}), inset: 1em, fill: {}.lighten(90%))[#text(weight: \"bold\", fill: {})[{}] ",
                        color, color, color,
                        callout_type.chars().next().unwrap().to_uppercase().to_string()
                            + &callout_type[1..]
                    );
                    self.output_buffer
                        .push_back(ParserEvent::Typst(TypstEvent::Raw(header.into())));

                    // Add content, skipping the prefix from first text
                    let mut first_text = true;
                    for event in self.blockquote_events.drain(..) {
                        match event {
                            ParserEvent::Typst(TypstEvent::Start(TypstTag::Quote(..)))
                            | ParserEvent::Typst(TypstEvent::End(TypstTag::Quote(..))) => {
                                // Skip quote start/end
                            }
                            ParserEvent::Typst(TypstEvent::Text(text)) if first_text => {
                                first_text = false;
                                // Strip the prefix from the first text
                                let content = text.as_ref().trim_start();
                                if let Some(stripped) = content.strip_prefix(prefix.as_str()) {
                                    let stripped = stripped.trim_start();
                                    if !stripped.is_empty() {
                                        self.output_buffer.push_back(ParserEvent::Typst(
                                            TypstEvent::Text(CowStr::from(stripped.to_string())),
                                        ));
                                    }
                                } else {
                                    self.output_buffer.push_back(ParserEvent::Typst(
                                        TypstEvent::Text(text),
                                    ));
                                }
                            }
                            _ => {
                                self.output_buffer.push_back(event);
                            }
                        }
                    }

                    // Close the block
                    self.output_buffer
                        .push_back(ParserEvent::Typst(TypstEvent::Raw("]\n".into())));

                    self.output_buffer.pop_front()
                } else {
                    // Not a callout, emit events as-is
                    for event in self.blockquote_events.drain(..) {
                        self.output_buffer.push_back(event);
                    }
                    self.output_buffer.pop_front()
                }
            }

            // Inside blockquote - buffer the event
            Some(event) if self.in_blockquote => {
                self.blockquote_events.push(event);
                self.next()
            }

            x => x,
        }
    }
}

/// Convert HTML anchor elements (`<a id="...">`) to Typst labels.
///
/// mdBook uses explicit HTML anchors for some cross-references that don't
/// correspond to headings. For example:
/// ```html
/// <a id="following-the-pointer-to-the-value-with-the-dereference-operator"></a>
/// ```
///
/// This converter extracts those IDs and emits them as Typst labels.
#[derive(Debug)]
pub struct ConvertHtmlAnchors<T> {
    iter: T,
}

impl<'a, T> ConvertHtmlAnchors<T>
where
    T: Iterator<Item = ParserEvent<'a>>,
{
    pub fn new(iter: T) -> Self {
        Self { iter }
    }
}

impl<'a, T> Iterator for ConvertHtmlAnchors<T>
where
    T: Iterator<Item = ParserEvent<'a>>,
{
    type Item = ParserEvent<'a>;

    fn next(&mut self) -> Option<Self::Item> {
        match self.iter.next() {
            // Handle block-level HTML
            Some(ParserEvent::Markdown(MdEvent::Html(html))) => {
                convert_html_to_typst(html.as_ref())
            }
            // Handle inline HTML (this is where <a id="..."> often appears)
            Some(ParserEvent::Markdown(MdEvent::InlineHtml(html))) => {
                convert_html_to_typst(html.as_ref())
            }
            // Handle mdbook wrapper for block HTML
            Some(ParserEvent::Mdbook(MdbookEvent::MarkdownContentEvent(MdEvent::Html(html)))) => {
                convert_html_to_typst(html.as_ref())
            }
            // Handle mdbook wrapper for inline HTML
            Some(ParserEvent::Mdbook(MdbookEvent::MarkdownContentEvent(MdEvent::InlineHtml(html)))) => {
                convert_html_to_typst(html.as_ref())
            }
            // Pass through all other events
            x => x,
        }
    }
}

/// Copy referenced assets (images) from source to destination as they are encountered.
///
/// This converter watches for image events and copies the referenced files
/// to the destination directory, preserving the directory structure.
pub struct CopyReferencedAssets<'a, T> {
    iter: T,
    source_dir: PathBuf,
    dest_dir: PathBuf,
    copied: HashSet<PathBuf>,
    _p: PhantomData<&'a ()>,
}

impl<'a, T> CopyReferencedAssets<'a, T>
where
    T: Iterator<Item = ParserEvent<'a>>,
{
    pub fn new(iter: T, source_dir: PathBuf, dest_dir: PathBuf) -> Self {
        Self {
            iter,
            source_dir,
            dest_dir,
            copied: HashSet::new(),
            _p: PhantomData,
        }
    }

    fn copy_asset(&mut self, path: &str) {
        // Skip URLs and absolute paths
        if path.starts_with("http://") || path.starts_with("https://") || path.starts_with('/') {
            return;
        }

        let asset_path = Path::new(path);
        let source_path = self.source_dir.join(asset_path);
        let dest_path = self.dest_dir.join(asset_path);

        // Skip if already copied
        if self.copied.contains(&source_path) {
            return;
        }

        // Check if source exists
        if !source_path.exists() {
            // Try without leading ./
            let cleaned = path.trim_start_matches("./");
            let source_path = self.source_dir.join(cleaned);
            let dest_path = self.dest_dir.join(cleaned);

            if source_path.exists() {
                self.do_copy(&source_path, &dest_path);
            }
            return;
        }

        self.do_copy(&source_path, &dest_path);
    }

    fn do_copy(&mut self, source: &Path, dest: &Path) {
        // Create parent directories if needed
        if let Some(parent) = dest.parent() {
            let _ = std::fs::create_dir_all(parent);
        }

        // Copy the file
        if let Err(e) = std::fs::copy(source, dest) {
            eprintln!("warning: failed to copy {}: {}", source.display(), e);
        } else {
            self.copied.insert(source.to_path_buf());
        }
    }
}

impl<'a, T> Iterator for CopyReferencedAssets<'a, T>
where
    T: Iterator<Item = ParserEvent<'a>>,
{
    type Item = ParserEvent<'a>;

    fn next(&mut self) -> Option<Self::Item> {
        let event = self.iter.next()?;

        // Check for image events and copy the referenced file
        match &event {
            // Typst image events
            ParserEvent::Typst(TypstEvent::Start(TypstTag::Image(src, _))) => {
                self.copy_asset(src.as_ref());
            }
            // Markdown image events
            ParserEvent::Markdown(MdEvent::Start(MdTag::Image { dest_url, .. })) => {
                self.copy_asset(dest_url.as_ref());
            }
            // mdBook wrapped markdown image events
            ParserEvent::Mdbook(MdbookEvent::MarkdownContentEvent(
                MdEvent::Start(MdTag::Image { dest_url, .. })
            )) => {
                self.copy_asset(dest_url.as_ref());
            }
            // HTML img tags (block-level)
            ParserEvent::Markdown(MdEvent::Html(html)) => {
                if let Some(src) = extract_img_src(html.as_ref()) {
                    self.copy_asset(&src);
                }
            }
            // HTML img tags (inline)
            ParserEvent::Markdown(MdEvent::InlineHtml(html)) => {
                if let Some(src) = extract_img_src(html.as_ref()) {
                    self.copy_asset(&src);
                }
            }
            // mdBook wrapped HTML img tags
            ParserEvent::Mdbook(MdbookEvent::MarkdownContentEvent(MdEvent::Html(html))) => {
                if let Some(src) = extract_img_src(html.as_ref()) {
                    self.copy_asset(&src);
                }
            }
            ParserEvent::Mdbook(MdbookEvent::MarkdownContentEvent(MdEvent::InlineHtml(html))) => {
                if let Some(src) = extract_img_src(html.as_ref()) {
                    self.copy_asset(&src);
                }
            }
            _ => {}
        }

        Some(event)
    }
}

/// Convert HTML to Typst markup.
/// Handles:
/// - `<a id="...">` -> Typst label
/// - `<img src="..." alt="...">` -> Typst image
/// - Other HTML -> comment
fn convert_html_to_typst(html: &str) -> Option<ParserEvent<'static>> {
    // Check for anchor with id
    if let Some(id) = extract_anchor_id(html) {
        return Some(ParserEvent::Typst(TypstEvent::Raw(
            format!("#[] <{}>\n", id).into(),
        )));
    }

    // Check for img tag
    if let Some(src) = extract_img_src(html) {
        let alt = extract_img_alt(html).unwrap_or_default();
        // Escape quotes in alt text
        let alt_escaped = alt.replace('"', "\\\"");
        return Some(ParserEvent::Typst(TypstEvent::Raw(
            format!("#image(\"{}\", alt: \"{}\")", src, alt_escaped).into(),
        )));
    }

    // Fall back to comment for other HTML
    Some(ParserEvent::Typst(TypstEvent::Raw(
        format!("/* HTML: {} */\n", html.replace("*/", "* /")).into(),
    )))
}

/// Extract the alt attribute from an HTML img tag.
fn extract_img_alt(html: &str) -> Option<String> {
    let html = html.trim();

    if !html.starts_with("<img ") {
        return None;
    }

    // Look for alt attribute
    if let Some(alt_start) = html.find("alt=") {
        let rest = &html[alt_start + 4..];
        let alt = if rest.starts_with('"') {
            rest[1..].split('"').next()
        } else if rest.starts_with('\'') {
            rest[1..].split('\'').next()
        } else {
            rest.split(|c| c == ' ' || c == '>').next()
        };

        alt.map(|s| s.to_string())
    } else {
        None
    }
}

/// Extract the src attribute from an HTML img tag.
fn extract_img_src(html: &str) -> Option<String> {
    let html = html.trim();

    // Match patterns like: <img src="foo.png"> or <img src='foo.png'>
    if !html.starts_with("<img ") {
        return None;
    }

    // Look for src attribute
    if let Some(src_start) = html.find("src=") {
        let rest = &html[src_start + 4..];
        let src = if rest.starts_with('"') {
            // Double quoted
            rest[1..].split('"').next()
        } else if rest.starts_with('\'') {
            // Single quoted
            rest[1..].split('\'').next()
        } else {
            // Unquoted - take until space or >
            rest.split(|c| c == ' ' || c == '>').next()
        };

        src.map(|s| s.to_string())
    } else {
        None
    }
}

/// Extract the id attribute from an HTML anchor tag.
fn extract_anchor_id(html: &str) -> Option<String> {
    let html = html.trim();

    // Match patterns like: <a id="foo"> or <a id='foo'> or <a id=foo>
    if !html.starts_with("<a ") {
        return None;
    }

    // Look for id attribute
    if let Some(id_start) = html.find("id=") {
        let rest = &html[id_start + 3..];
        let id = if rest.starts_with('"') {
            // Double quoted
            rest[1..].split('"').next()
        } else if rest.starts_with('\'') {
            // Single quoted
            rest[1..].split('\'').next()
        } else {
            // Unquoted - take until space or >
            rest.split(|c| c == ' ' || c == '>').next()
        };

        id.map(|s| s.to_string())
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    mod extract_img_src_tests {
        use super::*;

        #[test]
        fn double_quoted() {
            assert_eq!(
                extract_img_src(r#"<img src="img/ferris.svg" alt="Ferris"/>"#),
                Some("img/ferris.svg".to_string())
            );
        }

        #[test]
        fn single_quoted() {
            assert_eq!(
                extract_img_src("<img src='test.png' class='foo'>"),
                Some("test.png".to_string())
            );
        }

        #[test]
        fn not_an_img() {
            assert_eq!(extract_img_src("<a href='foo.png'>link</a>"), None);
        }

        #[test]
        fn no_src() {
            assert_eq!(extract_img_src("<img alt='test'>"), None);
        }
    }

    mod extract_anchor_id_tests {
        use super::*;

        #[test]
        fn double_quoted() {
            assert_eq!(
                extract_anchor_id(r#"<a id="test-anchor"></a>"#),
                Some("test-anchor".to_string())
            );
        }

        #[test]
        fn single_quoted() {
            assert_eq!(
                extract_anchor_id("<a id='test-anchor'></a>"),
                Some("test-anchor".to_string())
            );
        }

        #[test]
        fn not_an_anchor() {
            assert_eq!(extract_anchor_id("<p>text</p>"), None);
        }

        #[test]
        fn no_id() {
            assert_eq!(extract_anchor_id("<a href='#foo'></a>"), None);
        }
    }

    mod parse_code_info_tests {
        use super::*;

        #[test]
        fn empty_string() {
            let (lang, filename) = parse_code_info("");
            assert_eq!(lang, None);
            assert_eq!(filename, None);
        }

        #[test]
        fn language_only() {
            let (lang, filename) = parse_code_info("rust");
            assert_eq!(lang, Some("rust".to_string()));
            assert_eq!(filename, None);
        }

        #[test]
        fn language_with_ignore() {
            let (lang, filename) = parse_code_info("rust,ignore");
            assert_eq!(lang, Some("rust".to_string()));
            assert_eq!(filename, None);
        }

        #[test]
        fn language_with_multiple_attributes() {
            let (lang, filename) = parse_code_info("rust,ignore,no_run,editable");
            assert_eq!(lang, Some("rust".to_string()));
            assert_eq!(filename, None);
        }

        #[test]
        fn language_with_filename() {
            let (lang, filename) = parse_code_info("rust,filename=\"src/main.rs\"");
            assert_eq!(lang, Some("rust".to_string()));
            assert_eq!(filename, Some("src/main.rs".to_string()));
        }

        #[test]
        fn language_with_filename_and_other_attrs() {
            let (lang, filename) = parse_code_info("rust,ignore,filename=\"src/lib.rs\",editable");
            assert_eq!(lang, Some("rust".to_string()));
            assert_eq!(filename, Some("src/lib.rs".to_string()));
        }

        #[test]
        fn filename_with_single_quotes() {
            let (lang, filename) = parse_code_info("rust,filename='test.rs'");
            assert_eq!(lang, Some("rust".to_string()));
            assert_eq!(filename, Some("test.rs".to_string()));
        }

        #[test]
        fn filename_without_quotes() {
            let (lang, filename) = parse_code_info("rust,filename=main.rs");
            assert_eq!(lang, Some("rust".to_string()));
            assert_eq!(filename, Some("main.rs".to_string()));
        }

        #[test]
        fn whitespace_handling() {
            let (lang, filename) = parse_code_info(" rust , ignore , filename=\"foo.rs\" ");
            assert_eq!(lang, Some("rust".to_string()));
            assert_eq!(filename, Some("foo.rs".to_string()));
        }
    }

    mod detect_callout_type_tests {
        use super::*;

        #[test]
        fn note_prefix() {
            let result = detect_callout_type("Note: This is important");
            assert!(result.is_some());
            let (prefix, callout_type, _) = result.unwrap();
            assert_eq!(prefix, "Note:");
            assert_eq!(callout_type, "note");
        }

        #[test]
        fn warning_prefix() {
            let result = detect_callout_type("Warning: Be careful!");
            assert!(result.is_some());
            let (prefix, callout_type, _) = result.unwrap();
            assert_eq!(prefix, "Warning:");
            assert_eq!(callout_type, "warning");
        }

        #[test]
        fn tip_prefix() {
            let result = detect_callout_type("Tip: Try this");
            assert!(result.is_some());
            let (prefix, callout_type, _) = result.unwrap();
            assert_eq!(prefix, "Tip:");
            assert_eq!(callout_type, "tip");
        }

        #[test]
        fn important_prefix() {
            let result = detect_callout_type("Important: Read this");
            assert!(result.is_some());
            let (prefix, callout_type, _) = result.unwrap();
            assert_eq!(prefix, "Important:");
            assert_eq!(callout_type, "important");
        }

        #[test]
        fn no_prefix() {
            let result = detect_callout_type("Just some regular text");
            assert!(result.is_none());
        }

        #[test]
        fn leading_whitespace() {
            let result = detect_callout_type("   Note: With whitespace");
            assert!(result.is_some());
            let (prefix, _, _) = result.unwrap();
            assert_eq!(prefix, "Note:");
        }
    }
}
