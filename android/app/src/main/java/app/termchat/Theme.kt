package app.termchat

import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.darkColorScheme
import androidx.compose.runtime.Composable
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.sp

/** The look from the Brax Wink messenger (ADR-011, section 5). */
object Tc {
    val bg = Color(0xFF161616)
    val surface = Color(0xFF1E1F21)
    val line = Color(0xFF2C2D30)
    val mine = Color(0xFF3A4222)
    val theirs = Color(0xFF2A2B2F)
    val lime = Color(0xFFC3E66E)
    val text = Color(0xFFE8E8E6)
    val dim = Color(0xFF8C8D90)
    val warn = Color(0xFFF2A65A)
    val danger = Color(0xFFE5675F)
    val send = Color(0xFF242A35)

    val mono = TextStyle(fontFamily = FontFamily.Monospace, fontSize = 15.sp, color = text)
    val small = TextStyle(fontFamily = FontFamily.Monospace, fontSize = 12.sp, color = dim)
    val caps = TextStyle(fontFamily = FontFamily.Monospace, fontSize = 12.sp, letterSpacing = 1.sp, color = lime)
    val name = TextStyle(fontFamily = FontFamily.SansSerif, fontWeight = FontWeight.Bold, fontSize = 19.sp, color = text)
}

@Composable
fun TermchatTheme(content: @Composable () -> Unit) {
    MaterialTheme(
        colorScheme = darkColorScheme(
            primary = Tc.lime,
            onPrimary = Tc.bg,
            background = Tc.bg,
            surface = Tc.bg,
            surfaceVariant = Tc.surface,
            onSurface = Tc.text,
            onSurfaceVariant = Tc.dim,
            outline = Tc.line,
            error = Tc.danger,
        ),
        content = content,
    )
}
