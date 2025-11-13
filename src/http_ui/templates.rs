use hyper::StatusCode;
use maud::{html, Markup, PreEscaped, DOCTYPE};

use super::handlers::{BucketInfo, ObjectListResponse, ObjectMetadata};

/// Base HTML layout
fn layout(title: &str, content: Markup) -> Markup {
    html! {
        (DOCTYPE)
        html lang="en" {
            head {
                meta charset="utf-8";
                meta name="viewport" content="width=device-width, initial-scale=1";
                title { (title) }
                style { (PreEscaped(STYLES)) }
            }
            body {
                header {
                    h1 { "S3-CAS Browser" }
                    nav {
                        a href="/buckets" { "Buckets" }
                        " | "
                        a href="/health" { "Health" }
                    }
                }
                main {
                    (content)
                }
                footer {
                    p { "s3-cas v" (env!("CARGO_PKG_VERSION")) }
                }
            }
        }
    }
}

/// Bucket list page
pub fn buckets_page(buckets: &[BucketInfo]) -> String {
    let content = html! {
        div class="page-header" {
            h2 { "Buckets" }
            span class="count" { (buckets.len()) " bucket(s)" }
        }

        @if buckets.is_empty() {
            p class="empty-state" { "No buckets found" }
        } @else {
            table {
                thead {
                    tr {
                        th { "Name" }
                        th { "Created" }
                    }
                }
                tbody {
                    @for bucket in buckets {
                        tr {
                            td {
                                a href={ "/buckets/" (&bucket.name) } {
                                    (&bucket.name)
                                }
                            }
                            td { (&bucket.creation_date) }
                        }
                    }
                }
            }
        }
    };

    layout("Buckets - S3-CAS", content).into_string()
}

/// Object list page
pub fn objects_page(response: &ObjectListResponse) -> String {
    // Build breadcrumb navigation from prefix
    let breadcrumb_parts = if response.prefix.is_empty() {
        vec![]
    } else {
        response.prefix.trim_end_matches('/').split('/').collect()
    };

    let content = html! {
        div class="breadcrumb" {
            a href="/buckets" { "Buckets" }
            " / "
            a href={ "/buckets/" (response.bucket) } { (response.bucket) }
            @if !breadcrumb_parts.is_empty() {
                @for (i, part) in breadcrumb_parts.iter().enumerate() {
                    " / "
                    @if i == breadcrumb_parts.len() - 1 {
                        strong { (part) }
                    } @else {
                        @let prefix = breadcrumb_parts[..=i].join("/") + "/";
                        a href={ "/buckets/" (response.bucket) "?prefix=" (urlencoding::encode(&prefix)) } {
                            (part)
                        }
                    }
                }
            }
        }

        div class="page-header" {
            h2 {
                @if response.prefix.is_empty() {
                    "Objects in \"" (response.bucket) "\""
                } @else {
                    "\"" (response.prefix.trim_end_matches('/')) "\""
                }
            }
            span class="count" { (response.total_count) " item(s)" }
        }

        @if response.directories.is_empty() && response.objects.is_empty() {
            p class="empty-state" { "No objects in this location" }
        } @else {
            table {
                thead {
                    tr {
                        th { "Name" }
                        th class="number" { "Size" }
                        th { "Type" }
                        th { "Last Modified" }
                    }
                }
                tbody {
                    // Show directories first
                    @for dir in &response.directories {
                        tr class="directory-row" {
                            td {
                                a href={ "/buckets/" (response.bucket) "?prefix=" (urlencoding::encode(&dir.prefix)) } {
                                    "📁 " (dir.name)
                                }
                            }
                            td class="number" { "—" }
                            td { span class="badge directory" { "folder" } }
                            td { "—" }
                        }
                    }
                    // Show files
                    @for obj in &response.objects {
                        tr {
                            td {
                                a href={ "/buckets/" (response.bucket) "/" (obj.key) } {
                                    "📄 " (obj.key.rsplit('/').next().unwrap_or(&obj.key))
                                }
                            }
                            td class="number" { (format_size(obj.size)) }
                            td {
                                @if obj.is_inlined {
                                    span class="badge inline" { "inline" }
                                } @else {
                                    span class="badge blocks" { "blocks" }
                                }
                            }
                            td { (obj.last_modified) }
                        }
                    }
                }
            }
        }
    };

    layout(&format!("{} - S3-CAS", response.bucket), content).into_string()
}

/// Object detail page
pub fn object_detail_page(metadata: &ObjectMetadata) -> String {
    let content = html! {
        div class="breadcrumb" {
            a href="/buckets" { "← Buckets" }
            " / "
            a href={ "/buckets/" (metadata.bucket) } { (metadata.bucket) }
            " / "
            strong { (metadata.key) }
        }

        h2 { "Object Metadata" }

        dl class="metadata" {
            dt { "Key" }
            dd { code { (metadata.key) } }

            dt { "Bucket" }
            dd { code { (metadata.bucket) } }

            dt { "Size" }
            dd { (format_size(metadata.size)) " (" (metadata.size) " bytes)" }

            dt { "Content Hash (MD5)" }
            dd { code class="hash-full" { (metadata.hash) } }

            dt { "Last Modified" }
            dd { (metadata.last_modified) }

            dt { "Storage Type" }
            dd {
                @if metadata.is_inlined {
                    span class="badge inline" { "Inline" }
                    " (stored in metadata)"
                } @else {
                    span class="badge blocks" { "Blocks" }
                    " (content-addressable storage)"
                }
            }

            dt { "Block Count" }
            dd { (metadata.blocks.len()) }

            @if !metadata.blocks.is_empty() {
                dt { "Blocks" }
                dd {
                    table class="blocks-table" {
                        thead {
                            tr {
                                th { "#" }
                                th { "Hash" }
                                th class="number" { "Size" }
                                th class="number" { "Refcount" }
                            }
                        }
                        tbody {
                            @for (i, block) in metadata.blocks.iter().enumerate() {
                                tr {
                                    td { (i + 1) }
                                    td { code class="hash-full" { (block.hash) } }
                                    td class="number" { (format_size(block.size as u64)) }
                                    td class="number" {
                                        (block.refcount)
                                        @if block.refcount > 1 {
                                            " "
                                            span class="dedup-badge" { "shared" }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    };

    layout(&format!("{} - S3-CAS", metadata.key), content).into_string()
}

/// Error page
pub fn error_page(status: StatusCode, message: &str) -> String {
    let content = html! {
        div class="error-page" {
            h2 { "Error " (status.as_u16()) }
            p { (message) }
            p {
                a href="/buckets" { "← Back to buckets" }
            }
        }
    };

    layout(&format!("Error {} - S3-CAS", status.as_u16()), content).into_string()
}

// Helper functions

#[allow(dead_code)]
fn format_timestamp(time: std::time::SystemTime) -> String {
    use std::time::SystemTime;
    let duration = time
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default();
    let datetime = chrono::DateTime::from_timestamp(duration.as_secs() as i64, 0)
        .unwrap_or_default();
    datetime.format("%Y-%m-%d %H:%M:%S UTC").to_string()
}

fn format_size(bytes: u64) -> String {
    const UNITS: &[&str] = &["B", "KB", "MB", "GB", "TB"];
    let mut size = bytes as f64;
    let mut unit_idx = 0;

    while size >= 1024.0 && unit_idx < UNITS.len() - 1 {
        size /= 1024.0;
        unit_idx += 1;
    }

    if unit_idx == 0 {
        format!("{} {}", bytes, UNITS[0])
    } else {
        format!("{:.2} {}", size, UNITS[unit_idx])
    }
}

// CSS Styles
const STYLES: &str = r#"
* {
    margin: 0;
    padding: 0;
    box-sizing: border-box;
}

body {
    font-family: system-ui, -apple-system, BlinkMacSystemFont, 'Segoe UI', Roboto, sans-serif;
    line-height: 1.6;
    color: #333;
    background: #f5f5f5;
    padding-bottom: 3rem;
}

header {
    background: #2c3e50;
    color: white;
    padding: 1rem 2rem;
    display: flex;
    justify-content: space-between;
    align-items: center;
}

header h1 {
    font-size: 1.5rem;
    font-weight: 600;
}

nav a {
    color: #ecf0f1;
    text-decoration: none;
    font-size: 0.9rem;
}

nav a:hover {
    text-decoration: underline;
}

main {
    max-width: 1400px;
    margin: 2rem auto;
    padding: 0 2rem;
    background: white;
    border-radius: 8px;
    box-shadow: 0 1px 3px rgba(0,0,0,0.1);
    padding: 2rem;
}

footer {
    text-align: center;
    color: #666;
    font-size: 0.85rem;
    margin-top: 2rem;
}

.breadcrumb {
    color: #666;
    margin-bottom: 1.5rem;
    font-size: 0.9rem;
}

.breadcrumb a {
    color: #3498db;
    text-decoration: none;
}

.breadcrumb a:hover {
    text-decoration: underline;
}

.page-header {
    display: flex;
    justify-content: space-between;
    align-items: center;
    margin-bottom: 1.5rem;
    padding-bottom: 1rem;
    border-bottom: 2px solid #ecf0f1;
}

.page-header h2 {
    font-size: 1.75rem;
    color: #2c3e50;
}

.count {
    color: #7f8c8d;
    font-size: 0.9rem;
}

table {
    width: 100%;
    border-collapse: collapse;
    margin-top: 1rem;
}

th, td {
    text-align: left;
    padding: 0.4rem 0.6rem;
    border-bottom: 1px solid #ecf0f1;
}

th {
    background: #f8f9fa;
    font-weight: 600;
    color: #555;
    font-size: 0.9rem;
}

th.number, td.number {
    text-align: right;
}

tbody tr:hover {
    background: #f8f9fa;
}

tbody a {
    color: #3498db;
    text-decoration: none;
}

tbody a:hover {
    text-decoration: underline;
}

code {
    background: #f8f9fa;
    padding: 0.2rem 0.5rem;
    border-radius: 3px;
    font-size: 0.85rem;
    font-family: 'Courier New', monospace;
}

.hash-short {
    color: #7f8c8d;
}

.hash-full {
    word-break: break-all;
    font-size: 0.8rem;
}

.badge {
    display: inline-block;
    padding: 0.2rem 0.5rem;
    border-radius: 3px;
    font-size: 0.75rem;
    font-weight: 600;
    text-transform: uppercase;
}

.badge.inline {
    background: #e8f5e9;
    color: #2e7d32;
}

.badge.blocks {
    background: #e3f2fd;
    color: #1565c0;
}

.badge.directory {
    background: #fff3e0;
    color: #e65100;
}

.directory-row {
    font-weight: 500;
}

.directory-row:hover {
    background: #fffbf5;
}

.dedup-badge {
    background: #fff3e0;
    color: #e65100;
    padding: 0.1rem 0.3rem;
    border-radius: 3px;
    font-size: 0.7rem;
    font-weight: 600;
}

.metadata {
    background: #f8f9fa;
    padding: 1.5rem;
    border-radius: 6px;
    margin: 1.5rem 0;
}

.metadata dt {
    font-weight: 600;
    color: #555;
    margin-top: 1rem;
}

.metadata dt:first-child {
    margin-top: 0;
}

.metadata dd {
    margin: 0.5rem 0 0 0;
}

.blocks-table {
    margin-top: 0.5rem;
    background: white;
}

.blocks-table th {
    background: #ecf0f1;
}

.empty-state {
    text-align: center;
    color: #95a5a6;
    padding: 3rem 0;
    font-size: 1.1rem;
}

.error-page {
    text-align: center;
    padding: 3rem 0;
}

.error-page h2 {
    color: #e74c3c;
    margin-bottom: 1rem;
}

.error-page p {
    margin: 1rem 0;
}

.error-page a {
    color: #3498db;
    text-decoration: none;
}

.error-page a:hover {
    text-decoration: underline;
}

@media (max-width: 768px) {
    main {
        margin: 1rem;
        padding: 1rem;
    }

    header {
        flex-direction: column;
        text-align: center;
    }

    .page-header {
        flex-direction: column;
        align-items: flex-start;
    }

    table {
        font-size: 0.85rem;
    }

    th, td {
        padding: 0.5rem;
    }
}

@media (prefers-color-scheme: dark) {
    body {
        background: #1a1a1a;
        color: #e0e0e0;
    }

    main {
        background: #2d2d2d;
    }

    header {
        background: #1a1a1a;
    }

    th {
        background: #3a3a3a;
        color: #e0e0e0;
    }

    tbody tr:hover {
        background: #3a3a3a;
    }

    .directory-row:hover {
        background: #3a3a3a;
    }

    code, .metadata {
        background: #3a3a3a;
    }

    .breadcrumb {
        color: #a0a0a0;
    }

    .page-header h2 {
        color: #e0e0e0;
    }

    .count {
        color: #a0a0a0;
    }
}
"#;
