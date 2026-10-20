//! Sivryn patch: fonts bundled with the app.
//!
//! Blitz only loads `@font-face` rules from external stylesheets; rules in
//! inline `<style>` blocks (which Dioxus uses) are ignored. Apps register
//! their font files here before creating a document instead, and every new
//! document adds them to its font collection under their own family names.

use std::sync::Mutex;

static FONTS: Mutex<Vec<&'static [u8]>> = Mutex::new(Vec::new());

/// Adds a font file (TTF/OTF) for documents created from now on.
pub fn register(font: &'static [u8]) {
    FONTS.lock().unwrap_or_else(|e| e.into_inner()).push(font);
}

pub(crate) fn all() -> Vec<&'static [u8]> {
    FONTS.lock().unwrap_or_else(|e| e.into_inner()).clone()
}
