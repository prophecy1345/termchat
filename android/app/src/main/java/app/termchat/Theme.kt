package app.termchat

import android.app.Activity
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.darkColorScheme
import androidx.compose.material3.lightColorScheme
import androidx.compose.runtime.Composable
import androidx.compose.runtime.SideEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.luminance
import androidx.compose.ui.graphics.toArgb
import androidx.compose.ui.platform.LocalView
import androidx.core.view.WindowCompat
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.sp
import app.termchat.engine.themes

/** The theme the app starts with; the terminal client has the same list. */
const val DEFAULT_THEME = "termchat"

/** The current colours (ADR-011, section 5). A theme change redraws at once. */
object Tc {
    var theme by mutableStateOf(DEFAULT_THEME)
        private set
    var bg by mutableStateOf(Color(0xFF161616)); private set
    var surface by mutableStateOf(Color(0xFF1E1F21)); private set
    var line by mutableStateOf(Color(0xFF2C2D30)); private set
    var mine by mutableStateOf(Color(0xFF3A4222)); private set
    var theirs by mutableStateOf(Color(0xFF2A2B2F)); private set
    var lime by mutableStateOf(Color(0xFFC3E66E)); private set
    var text by mutableStateOf(Color(0xFFE8E8E6)); private set
    var dim by mutableStateOf(Color(0xFF8C8D90)); private set
    var warn by mutableStateOf(Color(0xFFF2A65A)); private set
    var danger by mutableStateOf(Color(0xFFE5675F)); private set
    var send by mutableStateOf(Color(0xFF2A2B2F)); private set

    val mono get() = TextStyle(fontFamily = FontFamily.Monospace, fontSize = 15.sp, color = text)
    val small get() = TextStyle(fontFamily = FontFamily.Monospace, fontSize = 12.sp, color = dim)
    val caps get() = TextStyle(fontFamily = FontFamily.Monospace, fontSize = 12.sp, letterSpacing = 1.sp, color = lime)
    val name get() = TextStyle(fontFamily = FontFamily.SansSerif, fontWeight = FontWeight.Bold, fontSize = 19.sp, color = text)

    /** Applies a theme by name; an unknown name falls back to the default. */
    fun use(wanted: String) {
        val all = themes()
        val t = all.firstOrNull { it.name == wanted } ?: all.first { it.name == DEFAULT_THEME }
        val c = t.colors.map { Color(0xFF000000 or it.toLong()) }
        // Slots, in order: accent, panel_bg, active_row_bg, selection_bg, surface0,
        // surface1, surface_dim, overlay0, overlay1, text, subtext0, mauve, green,
        // yellow, red, teal, peach.
        theme = t.name
        lime = c[0]; bg = c[1]; surface = c[2]; mine = c[3]; theirs = c[4]; send = c[4]
        line = c[5]; dim = c[8]; text = c[9]; danger = c[14]; warn = c[16]
    }
}

@Composable
fun TermchatTheme(content: @Composable () -> Unit) {
    val light = Tc.bg.luminance() > 0.5f
    val scheme = if (light) lightColorScheme() else darkColorScheme()
    // Dark status bar icons on a light theme, light ones on a dark theme.
    val view = LocalView.current
    SideEffect {
        (view.context as? Activity)?.window?.let {
            WindowCompat.getInsetsController(it, view).isAppearanceLightStatusBars = light
            it.decorView.setBackgroundColor(Tc.bg.toArgb())
        }
    }
    MaterialTheme(
        colorScheme = scheme.copy(
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
