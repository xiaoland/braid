use super::filter_html_comments;

#[test]
fn html_comments_preserve_surrounding_visible_markdown_bytes() {
    for (markdown, expected) in [
        ("<!-- hidden --> visible suffix", " visible suffix"),
        ("<!-- first -->visible<!-- second -->tail", "visibletail"),
        ("<!-- first -->\nvisible", "\nvisible"),
        ("before <!-- hidden --> after", "before  after"),
        ("前文 <!-- 隐藏 --> 后文\r\n", "前文  后文\r\n"),
        ("<!-- multi\nline --> suffix\nnext", " suffix\nnext"),
        ("<!-- first --> x < 3 <!-- hidden --> tail", " x < 3  tail"),
        ("<!-- first --> x </ 3 <!-- hidden --> tail", " x </ 3  tail"),
        (
            "<!-- hidden --><span title=\"<!-- literal -->\">visible</span><!-- hidden -->tail",
            "<span title=\"<!-- literal -->\">visible</span>tail",
        ),
        ("<!-- unfinished", ""),
    ] {
        assert_eq!(filter_html_comments(markdown), expected, "{markdown:?}");
    }
}

#[test]
fn comment_like_code_and_html_attribute_values_remain_literal() {
    for markdown in [
        "`<!-- literal -->`",
        "```html\n<!-- literal -->\n```",
        "    <!-- indented code -->\n",
        "<span title=\"<!-- literal -->\">visible</span>",
        "visible <!-- unfinished",
    ] {
        assert_eq!(filter_html_comments(markdown), markdown, "{markdown:?}");
    }
}
