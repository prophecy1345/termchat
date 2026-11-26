package app.termchat

import android.view.inputmethod.EditorInfo
import android.view.inputmethod.InputConnection
import androidx.compose.ui.ExperimentalComposeUiApi
import androidx.compose.ui.platform.InterceptPlatformTextInput
import androidx.compose.ui.platform.PlatformTextInputMethodRequest

import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.combinedClickable
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.BasicTextField
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.text.TextRange
import androidx.compose.ui.text.input.TextFieldValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.SolidColor
import androidx.compose.ui.draw.drawBehind
import androidx.compose.ui.geometry.Rect
import androidx.compose.ui.graphics.Path
import androidx.compose.ui.graphics.StrokeCap
import androidx.compose.ui.graphics.drawscope.Stroke
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.text.input.KeyboardType
import androidx.compose.ui.text.input.PasswordVisualTransformation
import androidx.compose.ui.text.input.VisualTransformation
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp
import androidx.compose.foundation.Canvas
import androidx.compose.material3.DropdownMenuItem
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.geometry.Size
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import app.termchat.engine.icons

val corner = RoundedCornerShape(6.dp)

/** Every field below asks the keyboard not to learn what is typed (ADR-011). */
@OptIn(ExperimentalComposeUiApi::class)
@Composable
fun NoLearning(content: @Composable () -> Unit) = InterceptPlatformTextInput(
    interceptor = { request, next ->
        next.startInputMethod(object : PlatformTextInputMethodRequest {
            override fun createInputConnection(outAttributes: EditorInfo): InputConnection {
                val connection = request.createInputConnection(outAttributes)
                outAttributes.imeOptions = outAttributes.imeOptions or EditorInfo.IME_FLAG_NO_PERSONALIZED_LEARNING
                return connection
            }
        })
    },
    content = content,
)

/** Square initials, like the reference's avatars. */
@Composable
fun Avatar(name: String, size: Dp = 42.dp) {
    // Only the name: after "·" comes the start of the fingerprint (ADR-007).
    val initials = name.substringBefore('·').split(' ').filter { it.isNotBlank() }.take(2)
        .joinToString("") { it.first().uppercase() }.ifEmpty { "?" }
    Box(
        Modifier.size(size).background(Tc.surface, RoundedCornerShape(4.dp)),
        contentAlignment = Alignment.Center,
    ) { Text(initials, style = Tc.small.copy(color = Tc.text)) }
}

/** A bordered monospace field. Never learns what is typed (ADR-011). */
@Composable
fun Field(
    value: String,
    onChange: (String) -> Unit,
    hint: String,
    modifier: Modifier = Modifier,
    secret: Boolean = false,
    singleLine: Boolean = true,
    // Taller text scrolls inside the field instead of pushing the screen away.
    maxLines: Int = 8,
) {
    // Text put in from outside (an edit, a clear) gets the cursor at its end.
    var field by remember { mutableStateOf(TextFieldValue(value, TextRange(value.length))) }
    if (field.text != value) field = TextFieldValue(value, TextRange(value.length))
    Field(
        field,
        {
            field = it
            if (it.text != value) onChange(it.text)
        },
        hint, modifier, secret, singleLine, maxLines,
    )
}

@Composable
internal fun Field(
    value: TextFieldValue,
    onChange: (TextFieldValue) -> Unit,
    hint: String,
    modifier: Modifier = Modifier,
    secret: Boolean = false,
    singleLine: Boolean = true,
    maxLines: Int = 8,
) {
    BasicTextField(
        value = value,
        onValueChange = onChange,
        singleLine = singleLine,
        maxLines = if (singleLine) 1 else maxLines,
        textStyle = Tc.mono,
        cursorBrush = SolidColor(Tc.lime),
        visualTransformation = if (secret) PasswordVisualTransformation() else VisualTransformation.None,
        keyboardOptions = KeyboardOptions(
            autoCorrectEnabled = false,
            keyboardType = if (secret) KeyboardType.Password else KeyboardType.Text,
        ),
        modifier = modifier.border(1.dp, Tc.line, corner).padding(horizontal = 14.dp, vertical = 13.dp),
        decorationBox = { inner ->
            Box(contentAlignment = Alignment.CenterStart) {
                if (value.text.isEmpty()) Text(hint, style = Tc.mono.copy(color = Tc.dim))
                inner()
            }
        },
    )
}

/** PLAN Q4: how much later a message can go, as the engine's `LATER`. */
internal val LATER = listOf(600L, 3600L, 3 * 3600L, 8 * 3600L, 86_400L)

/** "10m", "3h", "1d", as the engine's `span`. */
internal fun laterSpan(s: Long) = when {
    s < 3600 -> "${s / 60}m"
    s < 86_400 -> "${s / 3600}h"
    else -> "${s / 86_400}d"
}

/** When a message waiting for later goes: the local time, with the day if not today. */
internal fun laterAt(context: android.content.Context, at: ULong): String {
    val ms = at.toLong() * 1000
    val sameDay = android.text.format.DateUtils.isToday(ms)
    val flags = android.text.format.DateUtils.FORMAT_SHOW_TIME or if (sameDay) 0 else android.text.format.DateUtils.FORMAT_SHOW_DATE
    // A real context: the 12/24-hour setting is read from it (null crashed the app).
    return android.text.format.DateUtils.formatDateTime(context, ms, flags)
}

/** PLAN Q4: the messages waiting to go later, each with a cross to cancel. */
@Composable
internal fun LaterStrip(items: List<app.termchat.engine.LaterView>, cancel: (UInt) -> Unit) {
    val context = androidx.compose.ui.platform.LocalContext.current
    items.forEachIndexed { n, l ->
        Row(Modifier.fillMaxWidth().padding(start = 12.dp, end = 4.dp, top = 4.dp), verticalAlignment = Alignment.CenterVertically) {
            Text(
                "Later ${laterAt(context, l.at)} · ${l.text.lineSequence().first()}",
                style = Tc.small.copy(color = Tc.dim),
                maxLines = 1,
                overflow = androidx.compose.ui.text.style.TextOverflow.Ellipsis,
                modifier = Modifier.weight(1f),
            )
            IconTap("close", "Do not send it") { cancel(n.toUInt()) }
        }
    }
}

/** A flat capitals button, like SEND. */
@Composable
fun Flat(label: String, enabled: Boolean = true, color: Color = Tc.lime, modifier: Modifier = Modifier, onLong: (() -> Unit)? = null, onClick: () -> Unit) {
    Box(
        modifier
            .background(Tc.send, corner)
            .combinedClickable(enabled = enabled, onLongClick = onLong, onClick = onClick)
            .padding(horizontal = 18.dp, vertical = 13.dp),
        contentAlignment = Alignment.Center,
    ) { Text(label.uppercase(), style = Tc.caps.copy(color = if (enabled) color else Tc.dim)) }
}

@Composable
fun Line() = HorizontalDivider(color = Tc.line, thickness = 1.dp)

/** A row in a menu or the settings list. */
@Composable
fun MenuRow(label: String, detail: String = "", color: Color = Tc.text, dot: Boolean = false, onClick: () -> Unit) {
    Column(Modifier.fillMaxWidth().clickable(onClick = onClick).padding(horizontal = 16.dp, vertical = 14.dp)) {
        Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(10.dp)) {
            Text(label, style = Tc.mono.copy(color = color))
            if (dot) PixelIcon("dot", Tc.lime, 12.dp)
        }
        if (detail.isNotEmpty()) Text(detail, style = Tc.small)
    }
}

/** The shared icons (engine icons.rs): one-pixel line art in an 11×11 square. */
internal val ICONS: Map<String, ByteArray> = icons().associate { it.name to it.pixels }

/** Icon `name` in `color`; its pixels are whole screen pixels, so lines stay fine and sharp. */
@Composable
internal fun PixelIcon(name: String, color: Color, size: Dp = 14.dp, modifier: Modifier = Modifier) {
    val p = ICONS[name] ?: return
    Canvas(modifier.size(size)) {
        val px = kotlin.math.floor(this.size.width / 11f).coerceAtLeast(1f)
        val off = kotlin.math.floor((this.size.width - px * 11f) / 2f)
        for (k in 0 until p.size / 2) {
            drawRect(color, Offset(off + p[2 * k].toInt() * px, off + p[2 * k + 1].toInt() * px), Size(px, px))
        }
    }
}

/** An icon that is a button, named for TalkBack. */
@Composable
internal fun IconTap(name: String, hint: String, color: Color = Tc.text, onClick: () -> Unit) {
    Box(Modifier.semantics { contentDescription = hint }.clickable(onClick = onClick).padding(10.dp)) {
        PixelIcon(name, color, 16.dp)
    }
}

/** An icon, then a line of text. */
@Composable
internal fun IconText(name: String, text: String, style: TextStyle, color: Color, modifier: Modifier = Modifier, size: Dp = 14.dp) {
    Row(modifier, verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
        PixelIcon(name, color, size)
        Text(text, style = style.copy(color = color), maxLines = 1, overflow = androidx.compose.ui.text.style.TextOverflow.Ellipsis)
    }
}

/** A dropdown entry, with a dot when something waits behind it. */
@Composable
internal fun MenuItem(label: String, dot: Boolean = false, go: () -> Unit) {
    DropdownMenuItem(
        { Text(label, style = Tc.mono) }, go,
        trailingIcon = if (dot) ({ PixelIcon("dot", Tc.lime, 12.dp) }) else null,
    )
}

fun age(at: ULong): String {
    val s = (System.currentTimeMillis() / 1000 - at.toLong()).coerceAtLeast(0)
    return when {
        s < 60 -> "now"
        s < 3600 -> "${s / 60}m"
        s < 86_400 -> "${s / 3600}h"
        else -> "${s / 86_400}d"
    }
}

fun span(seconds: UInt?): String = when (val s = seconds?.toLong()) {
    null -> "off"
    in 0 until 3600 -> "${s / 60}m"
    in 3600 until 86_400 -> "${s / 3600}h"
    else -> "${s / 86_400}d"
}

val TextStyle.dim get() = copy(color = Tc.dim)

/** The attach button's icon: a paperclip drawn in the theme's line, like the
 *  timer's envelope, instead of the system's coloured emoji. */
@Composable
fun Paperclip(color: Color) {
    Box(Modifier.size(width = 14.dp, height = 22.dp).drawBehind {
        val u = size.width / 14f
        fun arc(cx: Float, cy: Float, r: Float, start: Float) =
            Rect(cx * u - r * u, cy * u - r * u, cx * u + r * u, cy * u + r * u) to start
        val clip = Path().apply {
            moveTo(5f * u, 8f * u)
            lineTo(5f * u, 16f * u)
            arc(7.5f, 16f, 2.5f, 180f).let { (r, a) -> arcTo(r, a, -180f, false) }
            lineTo(10f * u, 5f * u)
            arc(6.5f, 5f, 3.5f, 0f).let { (r, a) -> arcTo(r, a, -180f, false) }
            lineTo(3f * u, 17f * u)
            arc(7.5f, 17f, 4.5f, 180f).let { (r, a) -> arcTo(r, a, -180f, false) }
            lineTo(12f * u, 9f * u)
        }
        drawPath(clip, color, style = Stroke(width = 1.5.dp.toPx(), cap = StrokeCap.Round))
    })
}

/** A menu item that leaves its menu open: for a choice tapped through, as Mute's spans. */
@androidx.compose.runtime.Composable
internal fun KeepOpen(label: String, go: () -> Unit) {
    androidx.compose.material3.DropdownMenuItem({ androidx.compose.material3.Text(label, style = Tc.mono) }, go)
}
