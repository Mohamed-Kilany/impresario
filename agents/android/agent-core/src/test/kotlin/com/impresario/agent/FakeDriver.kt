package com.impresario.agent

/** Scriptable [UiDriver] for dispatcher tests (spec 11.3: the fake driver). */
class FakeDriver(
    var device: DeviceInfoData = DeviceInfoData(
        model = "Pixel 8",
        osName = "Android",
        osVersion = "15",
        screenWidthPx = 1080,
        screenHeightPx = 2400,
        pixelRatio = 2.75f,
        runtime = "UiAutomator2",
    ),
) : UiDriver, DriverExtras {

    class Node(
        val key: String,
        var text: String = "",
        var visible: Boolean = true,
        var enabled: Boolean = true,
        var focused: Boolean = false,
        var bounds: Rect = Rect(0, 0, 100, 100),
    )

    class Handle(val key: String) : ElementHandle

    val nodes = mutableListOf<Node>()

    /** selector `describe()` → node keys, in match order. */
    val selectorMatches = HashMap<String, MutableList<String>>()

    val calls = mutableListOf<String>()

    var crash = false
    var setTextIgnores = false
    var clearWorks = true
    var inputTextSucceeds = true
    var backgroundSucceeds = true
    var longPressKeySucceeds = true
    var screenshot = byteArrayOf(1, 2, 3, 4)

    /** Real-time block inside `find`, for cancellation tests. */
    var blockFindMs: Long = 0

    /** Hook invoked for every pressKey, so tests can move focus. */
    var onPressKey: ((Int) -> Unit)? = null

    fun node(key: String): Node = nodes.first { it.key == key }

    fun addNode(
        key: String,
        text: String = "",
        visible: Boolean = true,
        enabled: Boolean = true,
        focused: Boolean = false,
        bounds: Rect = Rect(0, 0, 100, 100),
        selector: String? = null,
    ): Node {
        val created = Node(key, text, visible, enabled, focused, bounds)
        nodes.add(created)
        (selector ?: key).let { key ->
            selectorMatches.getOrPut(key) { mutableListOf() }.add(created.key)
        }
        return created
    }

    private fun matched(sel: SelectorSpec): List<Handle> =
        selectorMatches[sel.describe()].orEmpty().map { Handle(it) }

    override fun find(sel: SelectorSpec, timeoutMs: Long): List<ElementHandle> {
        if (blockFindMs > 0) Thread.sleep(blockFindMs)
        calls.add("find:${sel.describe()}")
        return matched(sel)
    }

    override fun count(sel: SelectorSpec): Int = matched(sel).size

    override fun click(e: ElementHandle) {
        val handle = e as Handle
        calls.add("click:${handle.key}")
        val node = nodes.firstOrNull { it.key == handle.key } ?: return
        node.focused = true
    }

    override fun longClick(e: ElementHandle, ms: Long) {
        calls.add("longClick:${(e as Handle).key}:$ms")
    }

    override fun setText(e: ElementHandle, text: String) {
        calls.add("setText:${(e as Handle).key}:$text")
        if (!setTextIgnores) nodeOf(e).text = text
    }

    override fun text(e: ElementHandle): String = nodeOf(e).text

    override fun clear(e: ElementHandle) {
        calls.add("clear:${(e as Handle).key}")
        if (clearWorks) nodeOf(e).text = ""
    }

    override fun isVisible(e: ElementHandle): Boolean = nodeOf(e).visible

    override fun isEnabled(e: ElementHandle): Boolean = nodeOf(e).enabled

    override fun isFocused(e: ElementHandle): Boolean = nodeOf(e).focused

    override fun bounds(e: ElementHandle): Rect = nodeOf(e).bounds

    override fun focusedBounds(): Rect? =
        nodes.firstOrNull { it.focused }?.bounds

    override fun pressKey(code: Int, repeat: Int, intervalMs: Long) {
        calls.add("pressKey:$code:$repeat:$intervalMs")
        onPressKey?.invoke(code)
    }

    override fun pressBack() {
        calls.add("back")
    }

    /** Result of `scroll` calls; `false` models a container at its end. */
    var scrollMoves: Boolean = true

    /** Hook invoked for every `scroll`, so tests can reveal elements. */
    var onScroll: ((Direction) -> Unit)? = null

    override fun scroll(container: ElementHandle?, dir: Direction, steps: Int): Boolean {
        calls.add("scroll:${(container as? Handle)?.key ?: "-"}:${dir.name}:$steps")
        onScroll?.invoke(dir)
        return scrollMoves
    }

    override fun swipe(e: ElementHandle?, dir: Direction) {
        calls.add("swipe:${(e as? Handle)?.key ?: "-"}:${dir.name}")
    }

    override fun screenshotPng(): ByteArray = screenshot

    override fun launch(appId: String, tv: Boolean) {
        calls.add("launch:$appId:tv=$tv")
    }

    override fun terminate(appId: String) {
        calls.add("terminate:$appId")
    }

    override fun clearData(appId: String) {
        calls.add("clearData:$appId")
    }

    override fun deviceInfo(): DeviceInfoData = device

    // DriverExtras

    override fun inputText(text: String): Boolean {
        calls.add("inputText:$text")
        if (!inputTextSucceeds) return false
        val target = nodes.firstOrNull { it.focused } ?: return false
        target.text = target.text + text
        return true
    }

    override fun longPressKey(keyCode: Int, durationMs: Long): Boolean {
        calls.add("longPressKey:$keyCode:$durationMs")
        return longPressKeySucceeds
    }

    override fun putInBackground(appId: String): Boolean {
        calls.add("putInBackground:$appId")
        return backgroundSucceeds
    }

    override fun targetCrashed(appId: String): Boolean = crash

    private fun nodeOf(e: ElementHandle): Node {
        val key = (e as Handle).key
        return nodes.first { it.key == key }
    }
}
