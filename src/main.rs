//! Arkha — a Caja-style file manager in Rust + GTK 3.

// The window that puts all this together comes later.
#![allow(dead_code)]

mod config;
mod util;
#[cfg(test)]
mod testutil;
mod clipboard;
mod ops;
mod thumbnailers;
mod icons;
mod scroll;
mod folders;
mod iconlabel;
mod pane;
mod columns;
mod info;
mod props;
mod sidebar;
mod bulkrename;

fn main() {}
