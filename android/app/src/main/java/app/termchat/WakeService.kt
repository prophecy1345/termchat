package app.termchat

import android.os.Handler
import android.os.Looper
import org.unifiedpush.android.connector.FailedReason
import org.unifiedpush.android.connector.PushService
import org.unifiedpush.android.connector.data.PushEndpoint
import org.unifiedpush.android.connector.data.PushMessage

/**
 * UnifiedPush (ADR-012): the distributor hands us an endpoint, which goes to
 * the relay; a wake-up only says that something waits. A locked app has no
 * keys, so it does not connect: it shows "New message" and stops there.
 */
class WakeService : PushService() {
    private val model get() = (application as TermchatApp).model
    private val main = Handler(Looper.getMainLooper())

    override fun onNewEndpoint(endpoint: PushEndpoint, instance: String) {
        main.post { model.pushEndpoint(endpoint.url) }
    }

    override fun onMessage(message: PushMessage, instance: String) {
        main.post { model.woken() }
    }

    override fun onRegistrationFailed(reason: FailedReason, instance: String) {
        main.post { model.pushEndpoint(null); model.note = "push: the distributor said no ($reason)" }
    }

    override fun onUnregistered(instance: String) {
        main.post { model.pushEndpoint(null) }
    }
}
