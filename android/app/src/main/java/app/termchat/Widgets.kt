package app.termchat

import android.view.inputmethod.EditorInfo
import android.view.inputmethod.InputConnection
import androidx.compose.ui.ExperimentalComposeUiApi
import androidx.compose.ui.platform.InterceptPlatformTextInput
import androidx.compose.ui.platform.PlatformTextInputMethodRequest

import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.BasicTextField
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.SolidColor
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.text.input.KeyboardType
import androidx.compose.ui.text.input.PasswordVisualTransformation
import androidx.compose.ui.text.input.VisualTransformation
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp

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
) {
    BasicTextField(
        value = value,
        onValueChange = onChange,
        singleLine = singleLine,
        textStyle = Tc.mono,
        cursorBrush = SolidColor(Tc.lime),
        visualTransformation = if (secret) PasswordVisualTransformation() else VisualTransformation.None,
        keyboardOptions = KeyboardOptions(
            autoCorrectEnabled = false,
            keyboardType = if (secret) KeyboardType.Password else KeyboardType.Text,
        ),
        modifier = modifier.border(1.dp, Tc.line, corner).padding(horizontal = 14.dp, vertical = 13.dp),
        decorationBox = { inner ->
            Box {
                if (value.isEmpty()) Text(hint, style = Tc.mono.copy(color = Tc.dim))
                inner()
            }
        },
    )
}

/** A flat capitals button, like SEND. */
@Composable
fun Flat(label: String, enabled: Boolean = true, color: Color = Tc.lime, modifier: Modifier = Modifier, onClick: () -> Unit) {
    Box(
        modifier
            .background(Tc.send, corner)
            .clickable(enabled = enabled, onClick = onClick)
            .padding(horizontal = 18.dp, vertical = 13.dp),
        contentAlignment = Alignment.Center,
    ) { Text(label.uppercase(), style = Tc.caps.copy(color = if (enabled) color else Tc.dim)) }
}

@Composable
fun Line() = HorizontalDivider(color = Tc.line, thickness = 1.dp)

/** A row in a menu or the settings list. */
@Composable
fun MenuRow(label: String, detail: String = "", color: Color = Tc.text, onClick: () -> Unit) {
    Column(Modifier.fillMaxWidth().clickable(onClick = onClick).padding(horizontal = 16.dp, vertical = 14.dp)) {
        Text(label, style = Tc.mono.copy(color = color))
        if (detail.isNotEmpty()) Text(detail, style = Tc.small)
    }
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
