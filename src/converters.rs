use std::collections::{HashMap, HashSet, VecDeque};
use std::marker::PhantomData;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use pullup::markdown::{CodeBlockKind, CowStr, Event as MdEvent, Tag as MdTag, TagEnd as MdTagEnd};
use pullup::mdbook::{Event as MdbookEvent, Tag as MdbookTag};
use pullup::typst::{CodeBlockDisplay, Event as TypstEvent, Tag as TypstTag};
use pullup::ParserEvent;
use regex::Regex;

use crate::css::CssClassStyles;

/// Convert Markdown footnote definitions and references into Typst footnotes.
///
/// mdBook/pullup currently exposes these as ordinary text. We therefore do a
/// small, deliberately conservative pass over the generated Typst text. A
/// definition may span indented continuation lines, and references are
/// replaced only when their definition exists. Formatting produced by the
/// Markdown converter (quotes, images, emphasis, and so on) is retained as
/// Typst content instead of being flattened to plain text.
pub fn process_events<'a>(events: impl Iterator<Item = ParserEvent<'a>>) -> Vec<ParserEvent<'a>> {
    let definition_re =
        Regex::new(r"(?m)^[ \t]{0,3}\[\^([^\s\]]+)\]:[ \t]*(.*(?:\n(?:[ \t]{2,}|\t).*)*)")
            .expect("valid footnote definition regex");
    let reference_re = Regex::new(r"\[\^([^\s\]]+)\]").expect("valid footnote reference regex");

    let mut definitions = HashMap::new();
    let mut events = events.collect::<Vec<_>>();

    // Collect definitions first so references can appear before their source.
    for event in &mut events {
        if let ParserEvent::Typst(TypstEvent::Text(text)) = event {
            let (cleaned, found) = extract_footnote_definitions(text, &definition_re);
            definitions.extend(found);
            *text = cleaned.into();
        }
    }

    let mut final_events = Vec::with_capacity(events.len());
    for event in events {
        let event = match event {
            ParserEvent::Typst(TypstEvent::Text(text)) => {
                let updated = reference_re.replace_all(&text, |caps: &regex::Captures| {
                    definitions
                        .get(&caps[1])
                        .map(|content| format!("#footnote[{}]", content))
                        .unwrap_or_else(|| caps[0].to_string())
                });
                // TypstMarkup escapes raw `#` characters in text events. The
                // final output pass removes that escape for generated calls.
                yield_text(updated.into_owned())
            }
            other => other,
        };
        final_events.push(event);
    }

    final_events
}

fn extract_footnote_definitions(
    text: &str,
    definition_re: &Regex,
) -> (String, HashMap<String, String>) {
    let mut definitions = HashMap::new();
    let mut cleaned = String::with_capacity(text.len());
    let mut cursor = 0;

    for captures in definition_re.captures_iter(text) {
        let whole = captures.get(0).expect("footnote match");
        cleaned.push_str(&text[cursor..whole.start()]);
        let label = captures[1].to_string();
        let content = captures[2].trim().to_string();
        definitions.insert(label, footnote_content(content));
        cursor = whole.end();
    }

    cleaned.push_str(&text[cursor..]);
    (cleaned, definitions)
}

fn footnote_content(content: String) -> String {
    // Markdown that has already become Typst markup must remain executable so
    // quotes, images, emphasis, and figures continue to work. Plain text is
    // escaped to prevent a literal square bracket from terminating the
    // surrounding Typst content block.
    if content.contains('#') {
        content
    } else {
        content
            .replace('\\', r"\\")
            .replace('[', r"\[")
            .replace(']', r"\]")
    }
}

fn yield_text(text: String) -> ParserEvent<'static> {
    ParserEvent::Typst(TypstEvent::Text(text.into()))
}

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
                Some(ParserEvent::Typst(TypstEvent::Start(TypstTag::Heading(
                    num,
                    toc,
                    bookmarks,
                    label,
                )))),
            ) => Some(ParserEvent::Typst(TypstEvent::Start(TypstTag::Heading(
                num.saturating_add(1),
                toc,
                bookmarks,
                label,
            )))),
            (
                true,
                Some(ParserEvent::Typst(TypstEvent::End(TypstTag::Heading(
                    num,
                    toc,
                    bookmarks,
                    label,
                )))),
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

/// Fix heading stutter where mdBook emits duplicate headings.
/// When a chapter title and first heading have the same text, mdBook emits both.
/// This converter removes the duplicate but preserves any labels.
pub struct FixHeadingStutter<'a, T> {
    prev: Option<ParserEvent<'a>>,
    pending_label: Option<ParserEvent<'a>>,
    iter: T,
}

impl<'a, T> FixHeadingStutter<'a, T>
where
    T: Iterator<Item = ParserEvent<'a>>,
{
    #[allow(dead_code)]
    pub fn new(iter: T) -> Self {
        Self {
            prev: None,
            pending_label: None,
            iter,
        }
    }
}

impl<'a, T> Iterator for FixHeadingStutter<'a, T>
where
    T: Iterator<Item = ParserEvent<'a>>,
{
    type Item = ParserEvent<'a>;

    fn next(&mut self) -> Option<Self::Item> {
        // If we have a pending label to emit, return it first
        if let Some(label) = self.pending_label.take() {
            return Some(label);
        }

        match (&mut self.prev, self.iter.next()) {
            (
                Some(
                    event @ ParserEvent::Mdbook(MdbookEvent::MarkdownContentEvent(MdEvent::End(
                        MdTagEnd::Heading(_),
                    ))),
                ),
                Some(ParserEvent::Mdbook(MdbookEvent::MarkdownContentEvent(MdEvent::Start(
                    MdTag::Heading { .. },
                )))),
            ) => {
                self.prev = Some(event.clone());
                let _ = self.iter.find(|x| {
                    !matches!(
                        x,
                        ParserEvent::Mdbook(MdbookEvent::MarkdownContentEvent(MdEvent::End(
                            MdTagEnd::Heading(_)
                        ),)),
                    )
                });
                self.iter.next()
            }
            (
                event @ Some(ParserEvent::Typst(TypstEvent::End(TypstTag::Heading(..)))),
                Some(ParserEvent::Typst(TypstEvent::Start(TypstTag::Heading(_, _, _, label)))),
            ) => {
                self.prev = event.clone();
                // If the stutter heading has a label, preserve it as a standalone label
                if let Some(ref lbl) = label {
                    self.pending_label = Some(ParserEvent::Typst(TypstEvent::Raw(
                        format!("#[] <{}>\n", lbl).into(),
                    )));
                }
                // Skip to the end of this heading
                let _ = self.iter.find(|x| {
                    matches!(
                        x,
                        ParserEvent::Typst(TypstEvent::End(TypstTag::Heading(..)))
                    )
                });
                self.next()
            }
            (_, x) => {
                self.prev = x.clone();
                x
            }
        }
    }
}

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
                    self.buf
                        .push_back(ParserEvent::Typst(TypstEvent::Start(TypstTag::CodeBlock(
                            lang_cow, display,
                        ))));
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
                    self.buf
                        .push_back(ParserEvent::Typst(TypstEvent::Start(TypstTag::CodeBlock(
                            lang_cow,
                            CodeBlockDisplay::Block,
                        ))));
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
                    self.buf
                        .push_back(ParserEvent::Typst(TypstEvent::Start(TypstTag::CodeBlock(
                            lang_cow,
                            CodeBlockDisplay::Block,
                        ))));
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
        ("Note:", "note", "rgb(\"#1976D2\")"),           // Blue
        ("Warning:", "warning", "rgb(\"#F57C00\")"),     // Orange
        ("Tip:", "tip", "rgb(\"#388E3C\")"),             // Green
        ("Important:", "important", "rgb(\"#D32F2F\")"), // Red
        ("Caution:", "caution", "rgb(\"#F57C00\")"),     // Orange
        ("Info:", "info", "rgb(\"#1976D2\")"),           // Blue
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
                                    self.output_buffer
                                        .push_back(ParserEvent::Typst(TypstEvent::Text(text)));
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
/// It also handles `<span class="caption">` for figure captions.
pub struct ConvertHtmlAnchors<T> {
    iter: T,
    css_styles: Arc<CssClassStyles>,
    in_caption_span: bool,
    caption_content: String,
}

impl<'a, T> ConvertHtmlAnchors<T>
where
    T: Iterator<Item = ParserEvent<'a>>,
{
    pub fn new(iter: T, css_styles: Arc<CssClassStyles>) -> Self {
        Self {
            iter,
            css_styles,
            in_caption_span: false,
            caption_content: String::new(),
        }
    }
}

impl<'a, T> Iterator for ConvertHtmlAnchors<T>
where
    T: Iterator<Item = ParserEvent<'a>>,
{
    type Item = ParserEvent<'a>;

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            let event = self.iter.next();

            // Check if we're collecting caption content
            if self.in_caption_span {
                match &event {
                    // Text content while in caption span - collect it
                    Some(ParserEvent::Typst(TypstEvent::Text(text))) => {
                        self.caption_content.push_str(text.as_ref());
                        continue; // Keep collecting
                    }
                    // Code/raw content while in caption span
                    Some(ParserEvent::Typst(TypstEvent::Code(code))) => {
                        self.caption_content.push('`');
                        self.caption_content.push_str(code.as_ref());
                        self.caption_content.push('`');
                        continue;
                    }
                    // Check for closing </span>
                    Some(ParserEvent::Markdown(MdEvent::Html(html)))
                    | Some(ParserEvent::Markdown(MdEvent::InlineHtml(html)))
                        if html.trim() == "</span>" =>
                    {
                        self.in_caption_span = false;
                        let caption = std::mem::take(&mut self.caption_content);
                        // Emit styled caption
                        return Some(ParserEvent::Typst(TypstEvent::Raw(
                            format!("#align(center)[#emph[{}]]\n", caption).into(),
                        )));
                    }
                    Some(ParserEvent::Mdbook(MdbookEvent::MarkdownContentEvent(
                        MdEvent::Html(html),
                    )))
                    | Some(ParserEvent::Mdbook(MdbookEvent::MarkdownContentEvent(
                        MdEvent::InlineHtml(html),
                    ))) if html.trim() == "</span>" => {
                        self.in_caption_span = false;
                        let caption = std::mem::take(&mut self.caption_content);
                        return Some(ParserEvent::Typst(TypstEvent::Raw(
                            format!("#align(center)[#emph[{}]]\n", caption).into(),
                        )));
                    }
                    // Other events while in caption - skip them
                    Some(_) => continue,
                    None => {
                        // End of input while in caption - emit what we have
                        self.in_caption_span = false;
                        if !self.caption_content.is_empty() {
                            let caption = std::mem::take(&mut self.caption_content);
                            return Some(ParserEvent::Typst(TypstEvent::Raw(
                                format!("#align(center)[#emph[{}]]\n", caption).into(),
                            )));
                        }
                        return None;
                    }
                }
            }

            return match event {
                // Handle block-level HTML
                Some(ParserEvent::Markdown(MdEvent::Html(ref html))) => {
                    if is_caption_span_open(html.as_ref()) {
                        self.in_caption_span = true;
                        self.caption_content.clear();
                        continue; // Start collecting
                    }
                    convert_html_to_typst(html.as_ref(), &self.css_styles)
                }
                // Handle inline HTML
                Some(ParserEvent::Markdown(MdEvent::InlineHtml(ref html))) => {
                    if is_caption_span_open(html.as_ref()) {
                        self.in_caption_span = true;
                        self.caption_content.clear();
                        continue;
                    }
                    convert_html_to_typst(html.as_ref(), &self.css_styles)
                }
                // Handle mdbook wrapper for block HTML
                Some(ParserEvent::Mdbook(MdbookEvent::MarkdownContentEvent(MdEvent::Html(
                    ref html,
                )))) => {
                    if is_caption_span_open(html.as_ref()) {
                        self.in_caption_span = true;
                        self.caption_content.clear();
                        continue;
                    }
                    convert_html_to_typst(html.as_ref(), &self.css_styles)
                }
                // Handle mdbook wrapper for inline HTML
                Some(ParserEvent::Mdbook(MdbookEvent::MarkdownContentEvent(
                    MdEvent::InlineHtml(ref html),
                ))) => {
                    if is_caption_span_open(html.as_ref()) {
                        self.in_caption_span = true;
                        self.caption_content.clear();
                        continue;
                    }
                    convert_html_to_typst(html.as_ref(), &self.css_styles)
                }
                // Handle Typst Raw events that contain HTML comments
                Some(ParserEvent::Typst(TypstEvent::Raw(raw))) => {
                    if let Some(html) = extract_html_from_comment(raw.as_ref()) {
                        if is_caption_span_open(&html) {
                            self.in_caption_span = true;
                            self.caption_content.clear();
                            continue;
                        }
                        if let Some(converted) = convert_html_to_typst(&html, &self.css_styles) {
                            if !matches!(&converted, ParserEvent::Typst(TypstEvent::Raw(r)) if r.contains("/* HTML:"))
                            {
                                return Some(converted);
                            }
                        }
                    }
                    Some(ParserEvent::Typst(TypstEvent::Raw(raw)))
                }
                // Pass through all other events
                x => x,
            };
        }
    }
}

/// Check if HTML is an opening caption span tag.
fn is_caption_span_open(html: &str) -> bool {
    let html = html.trim();
    html.starts_with("<span ")
        && (html.contains("class=\"caption\"") || html.contains("class='caption'"))
        && !html.contains("</span>")
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
            ParserEvent::Typst(TypstEvent::Start(TypstTag::Image(src, ..))) => {
                self.copy_asset(src.as_ref());
            }
            // Markdown image events
            ParserEvent::Markdown(MdEvent::Start(MdTag::Image { dest_url, .. })) => {
                self.copy_asset(dest_url.as_ref());
            }
            // mdBook wrapped markdown image events
            ParserEvent::Mdbook(MdbookEvent::MarkdownContentEvent(MdEvent::Start(
                MdTag::Image { dest_url, .. },
            ))) => {
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
            // Typst Raw events containing HTML comments (from pullup's conversion)
            ParserEvent::Typst(TypstEvent::Raw(raw)) => {
                if let Some(html) = extract_html_from_comment(raw.as_ref()) {
                    if let Some(src) = extract_img_src(&html) {
                        self.copy_asset(&src);
                    }
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
/// - `<img src="..." alt="...">` -> Typst image with CSS-based sizing
/// - `<span class="caption">...</span>` -> Styled caption text
/// - Other HTML -> comment
fn convert_html_to_typst(html: &str, css_styles: &CssClassStyles) -> Option<ParserEvent<'static>> {
    let html_trimmed = html.trim();

    // Check for anchor with id
    if let Some(id) = extract_anchor_id(html) {
        return Some(ParserEvent::Typst(TypstEvent::Raw(
            format!("#[] <{}>\n", id).into(),
        )));
    }

    // Check for span with class="caption" - extract content and style it
    if let Some(caption) = extract_span_caption(html_trimmed) {
        // Render as centered, italic text with the figure number bold
        return Some(ParserEvent::Typst(TypstEvent::Raw(
            format!("#align(center)[#emph[{}]]\n", caption).into(),
        )));
    }

    // Check for closing </span> tag (ignore it, content was already handled)
    if html_trimmed == "</span>" {
        return Some(ParserEvent::Typst(TypstEvent::Raw("".into())));
    }

    // Check for img tag
    if let Some(src) = extract_img_src(html) {
        let alt = extract_img_alt(html).unwrap_or_default();
        // Escape quotes in alt text
        let alt_escaped = alt.replace('"', "\\\"");

        // Check for CSS class and get corresponding width/height from parsed CSS
        let class = extract_html_class(html);
        let width = class.as_ref().and_then(|c| css_styles.get_width(c));
        let height = class.as_ref().and_then(|c| css_styles.get_height(c));

        let img_call = match (width, height) {
            (Some(w), Some(h)) => {
                format!(
                    "#image(\"{}\", alt: \"{}\", width: {}, height: {})",
                    src, alt_escaped, w, h
                )
            }
            (Some(w), None) => {
                format!(
                    "#image(\"{}\", alt: \"{}\", width: {})",
                    src, alt_escaped, w
                )
            }
            (None, Some(h)) => {
                format!(
                    "#image(\"{}\", alt: \"{}\", height: {})",
                    src, alt_escaped, h
                )
            }
            (None, None) => {
                // No specific size from CSS - just use the image as-is
                format!("#image(\"{}\", alt: \"{}\")", src, alt_escaped)
            }
        };

        return Some(ParserEvent::Typst(TypstEvent::Raw(img_call.into())));
    }

    // Fall back to comment for other HTML
    Some(ParserEvent::Typst(TypstEvent::Raw(
        format!("/* HTML: {} */\n", html.replace("*/", "* /")).into(),
    )))
}

/// Extract HTML content from a Typst comment like `/* HTML: <img ...> */`.
fn extract_html_from_comment(comment: &str) -> Option<String> {
    let comment = comment.trim();
    if comment.starts_with("/* HTML:") && comment.ends_with("*/") {
        // Extract the HTML between "/* HTML:" and "*/"
        let html = &comment[8..comment.len() - 2];
        // Undo the escaping we did (replace "* /" back to "*/")
        let html = html.replace("* /", "*/").trim().to_string();
        if !html.is_empty() {
            return Some(html);
        }
    }
    None
}

/// Post-process Typst markup strings to replace HTML comments with proper Typst code.
/// This handles HTML that was embedded inside table cells or other constructs where
/// the event-based conversion couldn't intercept it.
pub fn process_html_comments(markup: &str, css_styles: &CssClassStyles) -> String {
    use regex::Regex;

    // Match /* HTML: <img ...> */ patterns
    let re = Regex::new(r"/\* HTML: (<img[^>]*/?>) \*/").unwrap();

    re.replace_all(markup, |caps: &regex::Captures| {
        let html = &caps[1];
        if let Some(src) = extract_img_src(html) {
            let alt = extract_img_alt(html).unwrap_or_default();
            let alt_escaped = alt.replace('"', "\\\"");

            let class = extract_html_class(html);
            let width = class.as_ref().and_then(|c| css_styles.get_width(c));
            let height = class.as_ref().and_then(|c| css_styles.get_height(c));

            match (width, height) {
                (Some(w), Some(h)) => {
                    format!(
                        "#image(\"{}\", alt: \"{}\", width: {}, height: {})",
                        src, alt_escaped, w, h
                    )
                }
                (Some(w), None) => {
                    format!(
                        "#image(\"{}\", alt: \"{}\", width: {})",
                        src, alt_escaped, w
                    )
                }
                (None, Some(h)) => {
                    format!(
                        "#image(\"{}\", alt: \"{}\", height: {})",
                        src, alt_escaped, h
                    )
                }
                (None, None) => {
                    format!("#image(\"{}\", alt: \"{}\")", src, alt_escaped)
                }
            }
        } else {
            // Not an img we can convert, leave it as-is
            caps[0].to_string()
        }
    })
    .to_string()
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
        let alt = if let Some(rest) = rest.strip_prefix('"') {
            rest.split('"').next()
        } else if let Some(rest) = rest.strip_prefix('\'') {
            rest.split('\'').next()
        } else {
            rest.split([' ', '>']).next()
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
        let src = if let Some(rest) = rest.strip_prefix('"') {
            // Double quoted
            rest.split('"').next()
        } else if let Some(rest) = rest.strip_prefix('\'') {
            // Single quoted
            rest.split('\'').next()
        } else {
            // Unquoted - take until space or >
            rest.split([' ', '>']).next()
        };

        src.map(|s| s.to_string())
    } else {
        None
    }
}

/// Extract the class attribute from an HTML tag.
fn extract_html_class(html: &str) -> Option<String> {
    let html = html.trim();

    // Look for class attribute
    if let Some(class_start) = html.find("class=") {
        let rest = &html[class_start + 6..];
        let class = if let Some(rest) = rest.strip_prefix('"') {
            rest.split('"').next()
        } else if let Some(rest) = rest.strip_prefix('\'') {
            rest.split('\'').next()
        } else {
            rest.split([' ', '>']).next()
        };

        class.map(|s| s.to_string())
    } else {
        None
    }
}

/// Resolve CSS classes on Image events to actual width/height values.
///
/// When pullup converts HTML img tags to Typst Image events, it preserves the
/// CSS class but not the resolved dimensions. This converter looks up the class
/// in the parsed CSS and substitutes the width/height values.
pub struct ResolveCssImageStyles<'a, T> {
    iter: T,
    css_styles: Arc<CssClassStyles>,
    _p: std::marker::PhantomData<&'a ()>,
}

impl<'a, T> ResolveCssImageStyles<'a, T>
where
    T: Iterator<Item = ParserEvent<'a>>,
{
    pub fn new(iter: T, css_styles: Arc<CssClassStyles>) -> Self {
        Self {
            iter,
            css_styles,
            _p: std::marker::PhantomData,
        }
    }
}

impl<'a, T> Iterator for ResolveCssImageStyles<'a, T>
where
    T: Iterator<Item = ParserEvent<'a>>,
{
    type Item = ParserEvent<'a>;

    fn next(&mut self) -> Option<Self::Item> {
        match self.iter.next() {
            // Handle Image Start events
            Some(ParserEvent::Typst(TypstEvent::Start(TypstTag::Image(
                src,
                alt,
                class,
                width,
                height,
            )))) => {
                // Resolve width/height from CSS class if we have a class and no dimensions
                let (resolved_width, resolved_height) = if let Some(ref c) = class {
                    let w = width
                        .clone()
                        .or_else(|| self.css_styles.get_width(c.as_ref()).map(CowStr::from));
                    let h = height
                        .clone()
                        .or_else(|| self.css_styles.get_height(c.as_ref()).map(CowStr::from));
                    (w, h)
                } else {
                    (width, height)
                };

                // Clear alt text for images with a class - these are HTML images that
                // typically have a separate <span class="caption"> for the actual caption.
                // Keeping the alt would cause Typst to wrap in #figure with auto-numbering.
                let resolved_alt = if class.is_some() {
                    CowStr::from("")
                } else {
                    alt
                };

                Some(ParserEvent::Typst(TypstEvent::Start(TypstTag::Image(
                    src,
                    resolved_alt,
                    class,
                    resolved_width,
                    resolved_height,
                ))))
            }
            // Handle Image End events (keep in sync with Start)
            Some(ParserEvent::Typst(TypstEvent::End(TypstTag::Image(
                src,
                alt,
                class,
                width,
                height,
            )))) => {
                let (resolved_width, resolved_height) = if let Some(ref c) = class {
                    let w = width
                        .clone()
                        .or_else(|| self.css_styles.get_width(c.as_ref()).map(CowStr::from));
                    let h = height
                        .clone()
                        .or_else(|| self.css_styles.get_height(c.as_ref()).map(CowStr::from));
                    (w, h)
                } else {
                    (width, height)
                };

                let resolved_alt = if class.is_some() {
                    CowStr::from("")
                } else {
                    alt
                };

                Some(ParserEvent::Typst(TypstEvent::End(TypstTag::Image(
                    src,
                    resolved_alt,
                    class,
                    resolved_width,
                    resolved_height,
                ))))
            }
            x => x,
        }
    }
}

/// Extract content from a `<span class="caption">...</span>` tag.
/// Returns the inner text content if this is a caption span.
fn extract_span_caption(html: &str) -> Option<String> {
    let html = html.trim();

    // Check if it starts with <span and has class="caption"
    if !html.starts_with("<span ") {
        return None;
    }

    // Check for class="caption"
    if !html.contains("class=\"caption\"") && !html.contains("class='caption'") {
        return None;
    }

    // Find the end of the opening tag
    let tag_end = html.find('>')?;
    let rest = &html[tag_end + 1..];

    // Find the closing </span>
    if let Some(close_pos) = rest.find("</span>") {
        let content = &rest[..close_pos];
        // Escape special Typst characters in the content
        let escaped = content
            .replace('#', "\\#")
            .replace('$', "\\$")
            .replace('<', "\\<")
            .replace('>', "\\>");
        Some(escaped.trim().to_string())
    } else {
        // Opening tag only - content will come in subsequent events
        // Return empty to signal we're in a caption span
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
        let id = if let Some(rest) = rest.strip_prefix('"') {
            // Double quoted
            rest.split('"').next()
        } else if let Some(rest) = rest.strip_prefix('\'') {
            // Single quoted
            rest.split('\'').next()
        } else {
            // Unquoted - take until space or >
            rest.split([' ', '>']).next()
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

#[cfg(test)]
mod html_comment_tests {
    use super::*;

    #[test]
    fn extract_from_comment() {
        let input = r#"/* HTML: <img src="img/ferris.svg" class="ferris-explain" alt="test"/> */"#;
        let result = extract_html_from_comment(input);
        assert_eq!(
            result,
            Some(r#"<img src="img/ferris.svg" class="ferris-explain" alt="test"/>"#.to_string())
        );
    }

    #[test]
    fn extract_from_comment_with_newline() {
        let input = "/* HTML: <img src=\"test.png\"/> */\n";
        let result = extract_html_from_comment(input);
        assert_eq!(result, Some("<img src=\"test.png\"/>".to_string()));
    }
}

#[cfg(test)]
mod process_html_comments_tests {
    use super::*;
    use std::sync::Arc;

    #[test]
    fn convert_ferris_img() {
        let css = Arc::new(CssClassStyles::new());
        let input =
            r#"[/* HTML: <img src="img/ferris/test.svg" class="ferris-explain" alt="Ferris"/> */]"#;
        let result = process_html_comments(input, &css);
        assert!(
            result.contains("#image"),
            "Expected #image, got: {}",
            result
        );
    }

    #[test]
    fn convert_self_closing_img() {
        let css = Arc::new(CssClassStyles::new());
        let input = r#"/* HTML: <img src="test.png" alt="test"/> */"#;
        let result = process_html_comments(input, &css);
        assert!(
            result.contains("#image"),
            "Expected #image, got: {}",
            result
        );
    }
}

#[cfg(test)]
mod footnote_tests {
    use super::*;

    fn definition_regex() -> Regex {
        Regex::new(r"(?m)^[ \t]{0,3}\[\^([^\s\]]+)\]:[ \t]*(.*(?:\n(?:[ \t]{2,}|\t).*)*)").unwrap()
    }

    #[test]
    fn extracts_multiline_definition_and_keeps_surrounding_text() {
        let input = "Before\n[^source]: First line\n  continuation\nAfter";
        let (cleaned, definitions) = extract_footnote_definitions(input, &definition_regex());

        assert_eq!(cleaned, "Before\n\nAfter");
        assert_eq!(definitions["source"], "First line\n  continuation");
    }

    #[test]
    fn escapes_square_brackets_in_plain_text() {
        assert_eq!(
            footnote_content("A [literal] note".into()),
            r"A \[literal\] note"
        );
    }

    #[test]
    fn preserves_typst_markup_for_quotes_and_images() {
        let content = "#quote[Quoted text] #image(\"figure.png\")".to_string();
        assert_eq!(footnote_content(content.clone()), content);
    }
}
