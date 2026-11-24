package app.termchat

import android.graphics.Bitmap
import androidx.compose.foundation.Image
import androidx.compose.foundation.background
import androidx.compose.foundation.gestures.detectTransformGestures
import androidx.compose.foundation.layout.*
import androidx.compose.material3.Text
import androidx.compose.runtime.*
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.FilterQuality
import androidx.compose.ui.graphics.asImageBitmap
import androidx.compose.ui.graphics.graphicsLayer
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.layout.ContentScale
import androidx.compose.ui.unit.dp
import androidx.compose.ui.window.Dialog
import androidx.compose.ui.window.DialogProperties
import androidx.compose.ui.window.SecureFlagPolicy
import app.termchat.engine.Picture
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext

/**
 * ADR-044: a received image, full screen, after an explicit tap. The Rust
 * engine decodes it (never BitmapFactory or ImageDecoder); here the pixels
 * only become a bitmap. Pinch to zoom, drag to move, back closes. The
 * bitmap is recycled on close and is never written anywhere.
 */
@Composable
fun ImageViewer(name: String, load: () -> Picture, onSave: () -> Unit, onClose: () -> Unit) {
    var bitmap by remember { mutableStateOf<Bitmap?>(null) }
    var error by remember { mutableStateOf("") }
    LaunchedEffect(Unit) {
        withContext(Dispatchers.Default) { runCatching { toBitmap(load()) } }
            .onSuccess { bitmap = it }
            .onFailure { error = it.message ?: "this image cannot be shown" }
    }
    DisposableEffect(Unit) { onDispose { bitmap?.recycle() } }
    var scale by remember { mutableFloatStateOf(1f) }
    var offset by remember { mutableStateOf(Offset.Zero) }
    Dialog(
        onDismissRequest = onClose,
        // The app's screenshot block covers the viewer too.
        properties = DialogProperties(usePlatformDefaultWidth = false, securePolicy = SecureFlagPolicy.Inherit),
    ) {
        Box(Modifier.fillMaxSize().background(Color.Black)) {
            val shown = bitmap
            when {
                shown != null -> Image(
                    shown.asImageBitmap(),
                    contentDescription = name,
                    contentScale = ContentScale.Fit,
                    filterQuality = FilterQuality.Medium,
                    modifier = Modifier
                        .fillMaxSize()
                        .pointerInput(Unit) {
                            detectTransformGestures { _, pan, zoom, _ ->
                                scale = (scale * zoom).coerceIn(1f, 8f)
                                offset = if (scale == 1f) Offset.Zero else offset + pan
                            }
                        }
                        .graphicsLayer(scaleX = scale, scaleY = scale, translationX = offset.x, translationY = offset.y),
                )
                error.isNotEmpty() -> Text(error, style = Tc.small.copy(color = Tc.warn), modifier = Modifier.align(Alignment.Center))
                else -> Text("…", style = Tc.small, modifier = Modifier.align(Alignment.Center))
            }
            Row(
                Modifier.align(Alignment.BottomCenter).padding(16.dp),
                horizontalArrangement = Arrangement.spacedBy(10.dp),
            ) {
                Flat("Save", color = Tc.lime) { onSave() }
                Flat("Close", color = Tc.dim) { onClose() }
            }
        }
    }
}

/** RGBA, not premultiplied, into an ARGB bitmap: no decoder involved. */
private fun toBitmap(p: Picture): Bitmap {
    val w = p.width.toInt()
    val h = p.height.toInt()
    val rgba = p.rgba
    val argb = IntArray(w * h) { n ->
        val i = n * 4
        ((rgba[i + 3].toInt() and 0xFF) shl 24) or
            ((rgba[i].toInt() and 0xFF) shl 16) or
            ((rgba[i + 1].toInt() and 0xFF) shl 8) or
            (rgba[i + 2].toInt() and 0xFF)
    }
    return Bitmap.createBitmap(argb, w, h, Bitmap.Config.ARGB_8888).also { argb.fill(0); rgba.fill(0) }
}
