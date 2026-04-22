#![allow(dead_code)]

use std::fs;
use std::path::{Path, PathBuf};

use quote::ToTokens;
use syn::{Attribute, File, ImplItem, Item, ItemImpl, Type};

pub fn collect_rust_files(root: &Path) -> Vec<PathBuf> {
    fn visit(dir: &Path, files: &mut Vec<PathBuf>) {
        let mut entries = fs::read_dir(dir)
            .expect("read_dir")
            .map(|entry| entry.expect("dir entry").path())
            .collect::<Vec<_>>();
        entries.sort();

        for entry in entries {
            if entry.is_dir() {
                visit(&entry, files);
            } else if entry.extension().is_some_and(|extension| extension == "rs") {
                files.push(entry);
            }
        }
    }

    let mut files = Vec::new();
    visit(root, &mut files);
    files
}

pub fn parse_rust_file(path: &Path) -> File {
    let contents = fs::read_to_string(path).expect("read source");
    syn::parse_file(&contents).expect("parse rust file")
}

pub fn production_tokens(path: &Path) -> String {
    let file = parse_rust_file(path);
    let mut tokens = Vec::new();
    for item in &file.items {
        collect_item_tokens(item, &mut tokens);
    }
    tokens.join("\n")
}

pub fn free_function_tokens(path: &Path, fn_name: &str) -> String {
    let file = parse_rust_file(path);
    file.items
        .iter()
        .find_map(|item| match item {
            Item::Fn(item_fn) if !has_cfg_test(&item_fn.attrs) && item_fn.sig.ident == fn_name => {
                Some(normalize_tokens(item_fn))
            }
            _ => None,
        })
        .unwrap_or_else(|| panic!("missing free function {fn_name}"))
}

pub fn trait_impl_method_tokens(
    path: &Path,
    trait_name: &str,
    self_ty: &str,
    fn_name: &str,
) -> String {
    let file = parse_rust_file(path);
    file.items
        .iter()
        .find_map(|item| match item {
            Item::Impl(item_impl)
                if !has_cfg_test(&item_impl.attrs)
                    && impl_trait_name(item_impl).as_deref() == Some(trait_name)
                    && impl_self_type(item_impl).as_deref() == Some(self_ty) =>
            {
                item_impl
                    .items
                    .iter()
                    .find_map(|impl_item| match impl_item {
                        ImplItem::Fn(method)
                            if !has_cfg_test(&method.attrs) && method.sig.ident == fn_name =>
                        {
                            Some(normalize_tokens(method))
                        }
                        _ => None,
                    })
            }
            _ => None,
        })
        .unwrap_or_else(|| panic!("missing impl method {fn_name}"))
}

fn normalize_tokens(tokens: impl ToTokens) -> String {
    tokens
        .to_token_stream()
        .to_string()
        .chars()
        .filter(|ch| !ch.is_whitespace())
        .collect()
}

fn collect_item_tokens(item: &Item, tokens: &mut Vec<String>) {
    if item_has_cfg_test(item) {
        return;
    }

    match item {
        Item::Impl(item_impl) => collect_impl_tokens(item_impl, tokens),
        Item::Mod(item_mod) => {
            if let Some((_, items)) = &item_mod.content {
                for nested in items {
                    collect_item_tokens(nested, tokens);
                }
            } else {
                tokens.push(normalize_tokens(item_mod));
            }
        }
        _ => tokens.push(normalize_tokens(item)),
    }
}

fn collect_impl_tokens(item_impl: &ItemImpl, tokens: &mut Vec<String>) {
    let mut header = String::new();
    if let Some((_, path, _)) = &item_impl.trait_ {
        header.push_str(&normalize_tokens(path));
    }
    header.push_str(&normalize_tokens(item_impl.self_ty.as_ref()));
    header.push_str(&normalize_tokens(&item_impl.generics));
    if !header.is_empty() {
        tokens.push(header);
    }

    for impl_item in &item_impl.items {
        if impl_item_has_cfg_test(impl_item) {
            continue;
        }
        tokens.push(normalize_tokens(impl_item));
    }
}

fn item_has_cfg_test(item: &Item) -> bool {
    match item {
        Item::Const(item) => has_cfg_test(&item.attrs),
        Item::Enum(item) => has_cfg_test(&item.attrs),
        Item::ExternCrate(item) => has_cfg_test(&item.attrs),
        Item::Fn(item) => has_cfg_test(&item.attrs),
        Item::ForeignMod(item) => has_cfg_test(&item.attrs),
        Item::Impl(item) => has_cfg_test(&item.attrs),
        Item::Macro(item) => has_cfg_test(&item.attrs),
        Item::Mod(item) => has_cfg_test(&item.attrs),
        Item::Static(item) => has_cfg_test(&item.attrs),
        Item::Struct(item) => has_cfg_test(&item.attrs),
        Item::Trait(item) => has_cfg_test(&item.attrs),
        Item::TraitAlias(item) => has_cfg_test(&item.attrs),
        Item::Type(item) => has_cfg_test(&item.attrs),
        Item::Union(item) => has_cfg_test(&item.attrs),
        Item::Use(item) => has_cfg_test(&item.attrs),
        _ => false,
    }
}

fn impl_item_has_cfg_test(item: &ImplItem) -> bool {
    match item {
        ImplItem::Const(item) => has_cfg_test(&item.attrs),
        ImplItem::Fn(item) => has_cfg_test(&item.attrs),
        ImplItem::Macro(item) => has_cfg_test(&item.attrs),
        ImplItem::Type(item) => has_cfg_test(&item.attrs),
        _ => false,
    }
}

fn has_cfg_test(attrs: &[Attribute]) -> bool {
    attrs.iter().any(|attr| {
        if !attr.path().is_ident("cfg") {
            return false;
        }

        let normalized = normalize_tokens(attr.meta.to_token_stream());
        normalized
            .strip_prefix("cfg(")
            .and_then(|body| body.strip_suffix(')'))
            .is_some_and(cfg_requires_test)
    })
}

fn cfg_requires_test(body: &str) -> bool {
    match body {
        "test" => true,
        _ if body.starts_with("all(") && body.ends_with(')') => {
            split_cfg_args(&body[4..body.len() - 1])
                .into_iter()
                .any(cfg_requires_test)
        }
        _ if body.starts_with("any(") && body.ends_with(')') => {
            split_cfg_args(&body[4..body.len() - 1])
                .into_iter()
                .all(cfg_requires_test)
        }
        _ => false,
    }
}

fn split_cfg_args(body: &str) -> Vec<&str> {
    let mut args = Vec::new();
    let mut depth = 0usize;
    let mut start = 0usize;

    for (index, ch) in body.char_indices() {
        match ch {
            '(' => depth += 1,
            ')' => depth = depth.saturating_sub(1),
            ',' if depth == 0 => {
                args.push(body[start..index].trim());
                start = index + 1;
            }
            _ => {}
        }
    }

    let tail = body[start..].trim();
    if !tail.is_empty() {
        args.push(tail);
    }
    args
}

fn impl_trait_name(item_impl: &ItemImpl) -> Option<String> {
    item_impl
        .trait_
        .as_ref()
        .and_then(|(_, path, _)| path.segments.last())
        .map(|segment| segment.ident.to_string())
}

fn impl_self_type(item_impl: &ItemImpl) -> Option<String> {
    match item_impl.self_ty.as_ref() {
        Type::Path(type_path) => type_path
            .path
            .segments
            .last()
            .map(|segment| segment.ident.to_string()),
        _ => None,
    }
}
