//! CSS parsing and translation to Typst styles.
//!
//! This module parses CSS files and extracts class-based styles that can be
//! translated to Typst properties.

use cssparser::{Parser, ParserInput, Token};
use std::collections::HashMap;
use std::fs;
use std::path::Path;

/// Represents CSS properties that can be translated to Typst.
#[derive(Debug, Clone, Default)]
pub struct CssStyles {
    pub width: Option<String>,
    pub height: Option<String>,
    pub color: Option<String>,
    pub background: Option<String>,
    pub background_color: Option<String>,
    pub font_size: Option<String>,
    pub font_weight: Option<String>,
    pub text_align: Option<String>,
}

impl CssStyles {
    /// Convert a CSS dimension value to Typst.
    pub fn css_dimension_to_typst(value: &str) -> String {
        // Typst uses similar units but with some differences
        // px -> pt (approximately), em -> em, % -> %
        let value = value.trim();
        if value.ends_with("px") {
            // Convert px to pt (roughly 1:1 for screen)
            let num = value.trim_end_matches("px");
            format!("{}pt", num)
        } else {
            // em, %, pt, etc. work as-is
            value.to_string()
        }
    }

    /// Get the width as a Typst-compatible value.
    pub fn typst_width(&self) -> Option<String> {
        self.width.as_ref().map(|w| Self::css_dimension_to_typst(w))
    }

    /// Get the height as a Typst-compatible value.
    pub fn typst_height(&self) -> Option<String> {
        self.height
            .as_ref()
            .map(|h| Self::css_dimension_to_typst(h))
    }
}

/// A lookup table of CSS class names to their styles.
#[derive(Debug, Default)]
pub struct CssClassStyles {
    classes: HashMap<String, CssStyles>,
}

impl CssClassStyles {
    /// Create a new empty style map.
    pub fn new() -> Self {
        Self {
            classes: HashMap::new(),
        }
    }

    /// Parse CSS from a string and add class styles to the map.
    pub fn parse_css(&mut self, css: &str) {
        let mut input = ParserInput::new(css);
        let mut parser = Parser::new(&mut input);

        while !parser.is_exhausted() {
            // Try to parse a rule
            if let Ok(selectors) = Self::parse_selectors(&mut parser) {
                if let Ok(styles) = Self::parse_declaration_block(&mut parser) {
                    // Add styles for each class selector
                    for selector in selectors {
                        if let Some(class_name) = selector.strip_prefix('.') {
                            // Handle compound selectors like ".class1.class2"
                            // Just use the first class for now
                            let class_name = class_name.split('.').next().unwrap_or(class_name);
                            self.classes.insert(class_name.to_string(), styles.clone());
                        }
                    }
                }
            } else {
                // Skip to next rule
                let _ = parser.next();
            }
        }
    }

    /// Parse selectors until we hit a `{`.
    fn parse_selectors(parser: &mut Parser) -> Result<Vec<String>, ()> {
        let mut selectors = Vec::new();
        let mut current = String::new();

        loop {
            let token = parser.next().map_err(|_| ())?;
            match token {
                Token::CurlyBracketBlock => {
                    if !current.trim().is_empty() {
                        selectors.push(current.trim().to_string());
                    }
                    return Ok(selectors);
                }
                Token::Comma => {
                    if !current.trim().is_empty() {
                        selectors.push(current.trim().to_string());
                    }
                    current = String::new();
                }
                Token::Delim(c) => {
                    current.push(*c);
                }
                Token::Ident(s) => {
                    current.push_str(s);
                }
                Token::Colon => {
                    current.push(':');
                }
                Token::WhiteSpace(_) => {
                    if !current.is_empty() {
                        current.push(' ');
                    }
                }
                _ => {
                    // Skip other tokens in selectors
                }
            }
        }
    }

    /// Parse a declaration block `{ property: value; ... }`.
    fn parse_declaration_block(parser: &mut Parser) -> Result<CssStyles, ()> {
        let mut styles = CssStyles::default();

        parser
            .parse_nested_block(|parser| {
                while !parser.is_exhausted() {
                    // Try to parse property: value;
                    if let Ok((property, value)) = Self::parse_declaration(parser) {
                        match property.as_str() {
                            "width" => styles.width = Some(value),
                            "height" => styles.height = Some(value),
                            "color" => styles.color = Some(value),
                            "background" => styles.background = Some(value),
                            "background-color" => styles.background_color = Some(value),
                            "font-size" => styles.font_size = Some(value),
                            "font-weight" => styles.font_weight = Some(value),
                            "text-align" => styles.text_align = Some(value),
                            _ => {}
                        }
                    }
                }
                Ok(())
            })
            .map_err(|_: cssparser::ParseError<'_, ()>| ())?;

        Ok(styles)
    }

    /// Parse a single declaration `property: value`.
    fn parse_declaration(parser: &mut Parser) -> Result<(String, String), ()> {
        // Get property name
        let property = match parser.next() {
            Ok(Token::Ident(s)) => s.to_string(),
            _ => return Err(()),
        };

        // Expect colon
        match parser.next() {
            Ok(Token::Colon) => {}
            _ => return Err(()),
        }

        // Collect value tokens until semicolon or end
        let mut value = String::new();
        loop {
            match parser.next() {
                Ok(Token::Semicolon) => break,
                Err(_) => break,
                Ok(Token::Ident(s)) => {
                    if !value.is_empty() {
                        value.push(' ');
                    }
                    value.push_str(s);
                }
                Ok(Token::Dimension { value: v, unit, .. }) => {
                    if !value.is_empty() {
                        value.push(' ');
                    }
                    value.push_str(&format!("{}{}", v, unit));
                }
                Ok(Token::Percentage { unit_value, .. }) => {
                    if !value.is_empty() {
                        value.push(' ');
                    }
                    value.push_str(&format!("{}%", unit_value * 100.0));
                }
                Ok(Token::Number { value: v, .. }) => {
                    if !value.is_empty() {
                        value.push(' ');
                    }
                    value.push_str(&format!("{}", v));
                }
                Ok(Token::Hash(s)) => {
                    if !value.is_empty() {
                        value.push(' ');
                    }
                    value.push('#');
                    value.push_str(s);
                }
                Ok(Token::Function(name)) => {
                    value.push_str(name);
                    value.push('(');
                    // Parse function arguments
                    let _ = parser.parse_nested_block(|parser| {
                        while let Ok(token) = parser.next() {
                            match token {
                                Token::Ident(s) => value.push_str(s),
                                Token::Dimension { value: v, unit, .. } => {
                                    value.push_str(&format!("{}{}", v, unit));
                                }
                                Token::Number { value: v, .. } => {
                                    value.push_str(&format!("{}", v));
                                }
                                Token::Comma => value.push_str(", "),
                                Token::WhiteSpace(_) => value.push(' '),
                                _ => {}
                            }
                        }
                        Ok::<_, cssparser::ParseError<'_, ()>>(())
                    });
                    value.push(')');
                }
                Ok(Token::WhiteSpace(_)) => {}
                _ => {}
            }
        }

        Ok((property, value.trim().to_string()))
    }

    /// Load and parse CSS from a file.
    pub fn load_css_file(&mut self, path: &Path) {
        if let Ok(css) = fs::read_to_string(path) {
            self.parse_css(&css);
        }
    }

    /// Load all CSS files from a directory.
    pub fn load_css_directory(&mut self, dir: &Path) {
        if let Ok(entries) = fs::read_dir(dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.extension().is_some_and(|e| e == "css") {
                    self.load_css_file(&path);
                }
            }
        }
    }

    /// Get styles for a class name.
    #[allow(dead_code)]
    pub fn get(&self, class: &str) -> Option<&CssStyles> {
        self.classes.get(class)
    }

    /// Get the Typst width for a class, if defined.
    pub fn get_width(&self, class: &str) -> Option<String> {
        self.classes.get(class).and_then(|s| s.typst_width())
    }

    /// Get the Typst height for a class, if defined.
    pub fn get_height(&self, class: &str) -> Option<String> {
        self.classes.get(class).and_then(|s| s.typst_height())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_simple_class() {
        let mut styles = CssClassStyles::new();
        styles.parse_css(
            r#"
            .ferris-explain {
                width: 100px;
            }
        "#,
        );

        let ferris = styles.get("ferris-explain").unwrap();
        assert_eq!(ferris.width, Some("100px".to_string()));
        assert_eq!(ferris.typst_width(), Some("100pt".to_string()));
    }

    #[test]
    fn parse_multiple_classes() {
        let mut styles = CssClassStyles::new();
        styles.parse_css(
            r#"
            .ferris-large {
                width: 4.5em;
            }
            .ferris-small {
                width: 2.3em;
            }
        "#,
        );

        assert_eq!(styles.get_width("ferris-large"), Some("4.5em".to_string()));
        assert_eq!(styles.get_width("ferris-small"), Some("2.3em".to_string()));
    }

    #[test]
    fn parse_multiple_properties() {
        let mut styles = CssClassStyles::new();
        styles.parse_css(
            r#"
            .my-image {
                width: 200px;
                height: 100px;
                background-color: #fff;
            }
        "#,
        );

        let img = styles.get("my-image").unwrap();
        assert_eq!(img.typst_width(), Some("200pt".to_string()));
        assert_eq!(img.typst_height(), Some("100pt".to_string()));
    }

    #[test]
    fn parse_ferris_css() {
        let mut styles = CssClassStyles::new();
        styles.parse_css(
            r#"
body.light .does_not_compile,
body.light .panics {
  background: #fff1f1;
}

.ferris-container {
  position: absolute;
  z-index: 99;
}

.ferris {
  vertical-align: top;
  margin-left: 0.2em;
  height: auto;
}

.ferris-large {
  width: 4.5em;
}

.ferris-small {
  width: 2.3em;
}

.ferris-explain {
  width: 100px;
}
        "#,
        );

        assert_eq!(
            styles.get_width("ferris-explain"),
            Some("100pt".to_string())
        );
        assert_eq!(styles.get_width("ferris-large"), Some("4.5em".to_string()));
        assert_eq!(styles.get_width("ferris-small"), Some("2.3em".to_string()));
    }
}
