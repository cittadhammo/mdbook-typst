# Tools Required

This document lists the required tools, their versions currently installed on this machine, where to find them, and how to install them.

## mdbook-typst

- **Version**: 0.1.6
- **Repository**: https://github.com/LegNeato/mdbook-typst

### Installation

#### Using cargo-binstall (recommended - fast, no compilation)

```sh
cargo install cargo-binstall
cargo binstall mdbook-typst
```

This downloads pre-compiled binaries instead of compiling from source.

#### From source (recommended for latest features)

```sh
git clone https://github.com/LegNeato/mdbook-typst.git
cd mdbook-typst
cargo install --path .
```

#### Pre-built binary

Download the latest release from the [GitHub releases](https://github.com/LegNeato/mdbook-typst/releases) page.

### Adding to PATH globally

After installation, add the binary to your PATH:

```sh
# Option 1: Using ~/.local/bin (no sudo required)
mkdir -p ~/.local/bin
cp target/release/mdbook-typst ~/.local/bin/
echo 'export PATH="$HOME/.local/bin:$PATH"' >> ~/.bashrc  # or ~/.zshrc

# Option 2: Using /usr/local/bin (requires sudo)
sudo cp target/release/mdbook-typst /usr/local/bin/
```

Verify installation:

```sh
mdbook-typst --version
```

---

## mdbook

- **Installed Version**: v0.4.40
- **Minimum Version**: 0.4.35 (compatible with mdbook-typst 0.1.6)
- **Repository**: https://github.com/rust-lang/mdBook

### Installation

```sh
cargo install mdbook
```

Or download pre-built binaries from the [mdBook releases](https://github.com/rust-lang/mdBook/releases).

Verify installation:

```sh
mdbook --version
```

---

## Typst CLI

- **Installed Version**: 0.14.2 (b33de9de)
- **Repository**: https://github.com/typst/typst

### Installation

#### From source (recommended)

```sh
cargo install --git https://github.com/typst/typst
```

#### Pre-built binaries

Download from the [Typst releases](https://github.com/typst/typst/releases).

#### Via package managers

- **macOS**: `brew install typst`
- **Linux**: Check your distribution's package manager or use Cargo

Verify installation:

```sh
typst --version
```

---

## Summary

| Tool | Installed Version | Installation Command |
|------|-------------------|---------------------|
| mdbook-typst | 0.1.6 | `cargo install --path .` |
| mdbook | v0.4.40 | `cargo install mdbook` |
| Typst CLI | 0.14.2 | `cargo install --git https://github.com/typst/typst` |
