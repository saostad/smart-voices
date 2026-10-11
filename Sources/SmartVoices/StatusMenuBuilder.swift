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

        let readFile = NSMenuItem(title: "Read File…", action: #selector(readFile(_:)), keyEquivalent: "o")
        readFile.target = self
        readFile.isEnabled = isIdle
        menu.addItem(readFile)

        let recentItem = NSMenuItem(title: "Recent", action: nil, keyEquivalent: "")
        let recentMenu = NSMenu()
        let recent = Array(history.items.prefix(10))
        if recent.isEmpty {
            let empty = NSMenuItem(title: "No recent items", action: nil, keyEquivalent: "")
            empty.isEnabled = false
            recentMenu.addItem(empty)
        } else {
            let formatter = RelativeDateTimeFormatter()
            formatter.dateTimeStyle = .named
            formatter.unitsStyle = .short
            let now = Date()
            for item in recent {
                let title =
                    "\(item.preview.prefix(50)) — \(formatter.localizedString(for: item.date, relativeTo: now))"
                let replay = NSMenuItem(
                    title: title, action: #selector(replayHistory(_:)), keyEquivalent: "")
                replay.target = self
                replay.representedObject = item.id.uuidString
                recentMenu.addItem(replay)
            }
        }
        recentItem.submenu = recentMenu
        menu.addItem(recentItem)

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

        let readerPanel = NSMenuItem(title: "Show Reading Panel", action: #selector(showReaderPanel(_:)), keyEquivalent: "p")
        readerPanel.target = self
        menu.addItem(readerPanel)

        let exportItem = NSMenuItem(title: "Export Clipboard as Audio…", action: #selector(exportClipboardAsAudio(_:)), keyEquivalent: "")
        exportItem.target = self
        exportItem.isEnabled = isIdle && !exporter.isExporting
        menu.addItem(exportItem)

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

        let sleepTitle: String
        if let left = sleepTimer.remaining {
            sleepTitle = "Sleep Timer (\(SleepTimer.format(remaining: left)) left)"
        } else {
            sleepTitle = "Sleep Timer"
        }
        let sleepItem = NSMenuItem(title: sleepTitle, action: nil, keyEquivalent: "")
        let sleepMenu = NSMenu()
        let off = NSMenuItem(title: "Off", action: #selector(setSleepTimer(_:)), keyEquivalent: "")
        off.target = self
        off.representedObject = 0
        off.state = sleepMinutes == nil ? .on : .off
        sleepMenu.addItem(off)
        for minutes in SleepTimer.menuMinutes {
            let item = NSMenuItem(
                title: "\(minutes) minutes", action: #selector(setSleepTimer(_:)),
                keyEquivalent: "")
            item.target = self
            item.representedObject = minutes
            item.state = sleepMinutes == minutes ? .on : .off
            sleepMenu.addItem(item)
        }
        sleepItem.submenu = sleepMenu
        menu.addItem(sleepItem)

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

        if let notice = playback.lastNotice {
            let full = "ⓘ \(notice)"
            let item = NSMenuItem(title: String(full.prefix(80)), action: nil, keyEquivalent: "")
            item.isEnabled = false
            item.toolTip = full
            menu.addItem(item)
            menu.addItem(.separator())
        }

        let settingsItem = NSMenuItem(title: "Settings…", action: #selector(openSettings(_:)), keyEquivalent: ",")
        settingsItem.target = self
        menu.addItem(settingsItem)

        let quit = NSMenuItem(title: "Quit Smart Voices", action: #selector(quitApp(_:)), keyEquivalent: "q")
        quit.target = self
        menu.addItem(quit)
        return menu
    }
}
