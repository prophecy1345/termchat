package app.termchat

import android.widget.Toast
import androidx.compose.foundation.background
import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.rememberScrollState
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.remember
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.text.LinkAnnotation
import androidx.compose.ui.text.SpanStyle
import androidx.compose.ui.text.TextLinkStyles
import androidx.compose.ui.text.TextRange
import androidx.compose.ui.text.buildAnnotatedString
import androidx.compose.ui.text.font.FontStyle
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.input.TextFieldValue
import androidx.compose.ui.text.style.TextDecoration
import androidx.compose.ui.text.withLink
import androidx.compose.ui.text.withStyle
import androidx.compose.ui.unit.dp
import app.termchat.engine.MarkdownBlock
import app.termchat.engine.MarkdownSpan
import app.termchat.engine.parseMarkdown

internal enum class Mark(val icon: String, val label: String) {
    Bold("bold", "Bold"), Italic("italic", "Italic"), Strike("strike", "Strikethrough"),
    Code("code", "Inline code"), CodeBlock("code_block", "Code block"), Link("link", "Link"),
    Quote("quote", "Quote"), List("list", "List");

    fun insert(field: TextFieldValue): TextFieldValue {
        val text = field.text
        val start = field.selection.min
        val end = field.selection.max
        val selected = text.substring(start, end)
        if (this == Quote || this == List) {
            val line = text.lastIndexOf('\n', start - 1) + 1
            val prefix = if (this == Quote) "> " else "- "
            val lines = text.substring(line, end)
            val body = lines.removeSuffix("\n").split('\n').joinToString("\n") { prefix + it }
            val inserted = body + if (lines.endsWith('\n')) "\n" else ""
            return TextFieldValue(text.replaceRange(line, end, inserted), TextRange(line + inserted.length))
        }
        val (before, after) = when (this) {
            Bold -> "**" to "**"
            Italic -> "*" to "*"
            Strike -> "~~" to "~~"
            Code -> "`" to "`"
            Link -> "[" to "]()"
            CodeBlock -> {
                val open = if (start == 0 || text[start - 1] == '\n') "```\n" else "\n```\n"
                val close = (if (selected.endsWith('\n')) "" else "\n") + "```" +
                    if (end == text.length || text[end] == '\n') "" else "\n"
                open to close
            }
            Quote, List -> error("line formatting handled above")
        }
        val cursor = end + before.length + if (this == Link && selected.isNotEmpty()) 2 else 0
        return TextFieldValue(text.replaceRange(start, end, before + selected + after), TextRange(cursor))
    }
}

@Composable
internal fun FormatBar(apply: (Mark) -> Unit) {
    Row(Modifier.fillMaxWidth().padding(horizontal = 12.dp).horizontalScroll(rememberScrollState())) {
        Mark.entries.forEach { mark ->
            IconTap(mark.icon, mark.label, color = Tc.lime) { apply(mark) }
        }
    }
}

/** Only the Rust engine interprets source text. Link clicks always copy plain text. */
@Composable
internal fun MarkdownText(text: String, quote: Boolean = false, links: Boolean = true) {
    val blocks = remember(text) { parseMarkdown(text) }
    val normal = if (quote) Tc.mono.copy(color = Tc.dim) else Tc.mono
    if (blocks == null) {
        Text(if (quote) text.lineSequence().first() else text, style = normal)
        return
    }
    val context = LocalContext.current
    val dim = Tc.dim
    val accent = Tc.lime
    val surface = Tc.surface
    val addressSize = Tc.small.fontSize
    Column {
        (if (quote) blocks.take(1) else blocks).forEach { block ->
            if (block is MarkdownBlock.Code) {
                Text(
                    if (quote) block.text.lineSequence().first() else block.text,
                    style = normal,
                    softWrap = false,
                    modifier = Modifier.background(surface, corner).horizontalScroll(rememberScrollState()).padding(4.dp),
                )
            } else {
                val (prefix, spans) = when (block) {
                    is MarkdownBlock.Line -> "" to block.spans
                    is MarkdownBlock.Quote -> "│ " to block.spans
                    is MarkdownBlock.Bullet -> "• " to block.spans
                    is MarkdownBlock.Numbered -> "${block.number}. " to block.spans
                    is MarkdownBlock.Code -> error("code blocks handled above")
                }
                Text(buildAnnotatedString {
                    withStyle(SpanStyle(color = dim)) { append(prefix) }
                    spans.forEach { span ->
                        when (span) {
                            is MarkdownSpan.Plain -> append(span.text)
                            is MarkdownSpan.Bold -> withStyle(SpanStyle(fontWeight = FontWeight.Bold)) { append(span.text) }
                            is MarkdownSpan.Italic -> withStyle(SpanStyle(fontStyle = FontStyle.Italic)) { append(span.text) }
                            is MarkdownSpan.Strike -> withStyle(SpanStyle(textDecoration = TextDecoration.LineThrough)) { append(span.text) }
                            is MarkdownSpan.Code -> withStyle(SpanStyle(background = surface)) { append(span.text) }
                            is MarkdownSpan.Link -> {
                                val content = {
                                    withStyle(SpanStyle(color = accent, textDecoration = TextDecoration.Underline)) { append(span.label) }
                                    withStyle(SpanStyle(color = dim, fontSize = addressSize)) { append(" <${span.address}>") }
                                }
                                if (links) withLink(LinkAnnotation.Clickable(
                                    tag = span.address,
                                    styles = TextLinkStyles(focusedStyle = SpanStyle(background = surface)),
                                    linkInteractionListener = {
                                        copySensitive(context, span.address)
                                        Toast.makeText(context, "Address copied", Toast.LENGTH_SHORT).show()
                                    },
                                )) { content() } else content()
                            }
                        }
                    }
                    if (length == 0) append(" ")
                }, style = normal)
            }
        }
    }
}
