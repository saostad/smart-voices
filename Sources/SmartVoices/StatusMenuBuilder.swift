import AppKit
import Foundation
import SmartVoicesCore

extension AppDelegate {
    func buildMenu() -> NSMenu {
        let menu = NSMenu()
        let isIdle = playback.state == .idle
        let isPaused = playback.state == .paused
        let isActive = !isIdle

        let read = NSMenuItem(title: "Read Clipboard", action: #selector(readClipboard(_:)), keyEquivalent: "r")
        read.target = self
        read.isEnabled = isIdle
        menu.addItem(read)

        let pauseTitle = isPaused ? "Resume" : "Pause"
        let pauseItem = NSMenuItem(title: pauseTitle, action: #selector(togglePause(_:)), keyEquivalent: "")
        pauseItem.target = self
        pauseItem.isEnabled = isActive
        menu.addItem(pauseItem)

        let stop = NSMenuItem(title: "Stop", action: #selector(stopPlayback(_:)), keyEquivalent: "")
        stop.target = self
        stop.isEnabled = isActive
        menu.addItem(stop)

        let previous = NSMenuItem(title: "Previous Sentence", action: #selector(previousSentence(_:)), keyEquivalent: "")
        previous.target = self
        previous.isEnabled = isActive && playback.currentChunkIndex > 0
        menu.addItem(previous)

        let next = NSMenuItem(title: "Next Sentence", action: #selector(nextSentence(_:)), keyEquivalent: "")
        next.target = self
        next.isEnabled = isActive && playback.currentChunkIndex < playback.chunks.count - 1
        menu.addItem(next)

        menu.addItem(.separator())

        let speedItem = NSMenuItem(title: "Speed", action: nil, keyEquivalent: "")
        let speedMenu = NSMenu()
        for value in [0.75, 1.0, 1.25, 1.5, 1.75, 2.0] {
            let label: String
            if value == 1.0 {
                label = "1\u{00D7}"
            } else if value == 0.75 {
                label = "0.75\u{00D7}"
            } else if value == 1.25 {
                label = "1.25\u{00D7}"
            } else if value == 1.5 {
                label = "1.5\u{00D7}"
            } else if value == 1.75 {
                label = "1.75\u{00D7}"
            } else {
                label = "2\u{00D7}"
            }
            let item = NSMenuItem(title: label, action: #selector(setSpeed(_:)), keyEquivalent: "")
            item.target = self
            item.representedObject = value
            item.state = playback.speed == value ? .on : .off
            speedMenu.addItem(item)
        }
        speedItem.submenu = speedMenu
        menu.addItem(speedItem)

        let voiceItem = NSMenuItem(title: "Voice", action: nil, keyEquivalent: "")
        let voiceMenu = NSMenu()
        if cachedVoices.isEmpty {
            let loading = NSMenuItem(title: "Loading voices…", action: nil, keyEquivalent: "")
            loading.isEnabled = false
            voiceMenu.addItem(loading)
        } else {
            let currentID = voiceForSettings()
            for voice in cachedVoices {
                let item = NSMenuItem(title: voice.name, action: #selector(setVoice(_:)), keyEquivalent: "")
                item.target = self
                item.representedObject = voice.id
                item.state = voice.id == currentID ? .on : .off
                voiceMenu.addItem(item)
            }
        }
        voiceItem.submenu = voiceMenu
        menu.addItem(voiceItem)

        let engineItem = NSMenuItem(title: "Engine", action: nil, keyEquivalent: "")
        let engineMenu = NSMenu()
        for (title, id) in [("Kokoro (natural)", "kokoro"), ("macOS Voice", "system")] {
            let item = NSMenuItem(title: title, action: #selector(setEngine(_:)), keyEquivalent: "")
            item.target = self
            item.representedObject = id
            item.state = settings.engineID == id ? .on : .off
            engineMenu.addItem(item)
        }
        engineItem.submenu = engineMenu
        menu.addItem(engineItem)

        if engineLoadFailed && settings.engineID == "kokoro" {
            let fallback = NSMenuItem(title: "Use macOS Voice Instead", action: #selector(setEngine(_:)), keyEquivalent: "")
            fallback.target = self
            fallback.representedObject = "system"
            menu.addItem(fallback)
        }

        menu.addItem(.separator())

        if let error = playback.lastError {
            let full = "\u{26A0}\u{FE0E} \(error)"
            let item = NSMenuItem(title: String(full.prefix(80)), action: nil, keyEquivalent: "")
            item.isEnabled = false
            item.toolTip = full
            menu.addItem(item)
            menu.addItem(.separator())
        }

        let quit = NSMenuItem(title: "Quit Smart Voices", action: #selector(quitApp(_:)), keyEquivalent: "q")
        quit.target = self
        menu.addItem(quit)
        return menu
    }
}
