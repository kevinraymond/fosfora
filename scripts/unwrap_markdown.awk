# Rejoin hard-wrapped Markdown so each paragraph and list item is one line.
#
# CHANGELOG.md is wrapped at ~90 columns so it reads well in an editor, and GitHub
# rejoins those lines when it renders a file. A RELEASE BODY is rendered differently:
# every newline is a line break, so a wrapped entry came out as ragged short lines.
# release.yml pipes each version's notes through this before publishing.
#
# A line continues the one before it unless it starts something of its own: a list
# item (any depth, so nested bullets stay nested), a heading, an HTML tag, a table row,
# or a blank line. Continuations lose their leading indent and join with one space.
#
#   awk -f scripts/unwrap_markdown.awk notes.md

function flush() {
    if (buf != "") print buf
    buf = ""
}

/^[[:space:]]*$/                             { flush(); print; next }
/^#/ || /^[[:space:]]*</ || /^[[:space:]]*\|/ { flush(); print; next }
/^[[:space:]]*([-*+]|[0-9]+\.)[[:space:]]/  { flush(); buf = $0; next }
{
    line = $0
    if (buf != "") {
        sub(/^[[:space:]]+/, "", line)
        buf = buf " " line
    } else {
        buf = line
    }
}
END { flush() }
