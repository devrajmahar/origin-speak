//! Global shortcut registration independent of the desktop transport.
//!
//! Native frontends consume these typed events directly on their application
//! event loop.

use global_hotkey::{hotkey::HotKey, GlobalHotKeyEvent, GlobalHotKeyManager, HotKeyState};
use std::{
    str::FromStr,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, RwLock,
    },
    thread::JoinHandle,
    time::Duration,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShortcutEvent {
    TriggerPressed,
    TriggerReleased,
    AssistantPressed,
}

#[derive(Clone, Copy, Default)]
struct ShortcutIds {
    trigger: u32,
    assistant: u32,
}

/// Owns the two application-wide hotkeys used by ListenOS.
///
/// The callback runs on the global-hotkey event thread. Frontends should hand
/// the event off to their own event loop rather than doing UI work inline.
pub struct ShortcutService {
    manager: GlobalHotKeyManager,
    trigger: HotKey,
    assistant: HotKey,
    ids: Arc<RwLock<ShortcutIds>>,
    running: Arc<AtomicBool>,
    event_thread: Option<JoinHandle<()>>,
}

impl ShortcutService {
    pub fn new<F>(trigger: &str, assistant: &str, on_event: F) -> Result<Self, String>
    where
        F: Fn(ShortcutEvent) + Send + 'static,
    {
        let manager = GlobalHotKeyManager::new().map_err(|error| error.to_string())?;
        let trigger = HotKey::from_str(trigger).map_err(|error| error.to_string())?;
        let assistant = HotKey::from_str(assistant).map_err(|error| error.to_string())?;

        manager
            .register(trigger)
            .map_err(|error| error.to_string())?;
        if let Err(error) = manager.register(assistant) {
            let _ = manager.unregister(trigger);
            return Err(error.to_string());
        }

        let ids = Arc::new(RwLock::new(ShortcutIds {
            trigger: trigger.id(),
            assistant: assistant.id(),
        }));
        let event_ids = ids.clone();
        let running = Arc::new(AtomicBool::new(true));
        let event_running = running.clone();

        let event_thread = std::thread::spawn(move || {
            while event_running.load(Ordering::Acquire) {
                let Ok(event) =
                    GlobalHotKeyEvent::receiver().recv_timeout(Duration::from_millis(100))
                else {
                    continue;
                };
                if !event_running.load(Ordering::Acquire) {
                    break;
                }
                let current = event_ids.read().map(|guard| *guard).unwrap_or_default();
                let shortcut_event = if event.id == current.trigger {
                    match event.state {
                        HotKeyState::Pressed => Some(ShortcutEvent::TriggerPressed),
                        HotKeyState::Released => Some(ShortcutEvent::TriggerReleased),
                    }
                } else if event.id == current.assistant && event.state == HotKeyState::Pressed {
                    Some(ShortcutEvent::AssistantPressed)
                } else {
                    None
                };

                if let Some(shortcut_event) = shortcut_event {
                    on_event(shortcut_event);
                }
            }
        });

        Ok(Self {
            manager,
            trigger,
            assistant,
            ids,
            running,
            event_thread: Some(event_thread),
        })
    }

    pub fn update(&mut self, trigger: &str, assistant: &str) -> Result<(), String> {
        let next_trigger = HotKey::from_str(trigger).map_err(|error| error.to_string())?;
        let next_assistant = HotKey::from_str(assistant).map_err(|error| error.to_string())?;

        self.manager
            .unregister(self.trigger)
            .map_err(|error| error.to_string())?;
        if let Err(error) = self.manager.unregister(self.assistant) {
            let _ = self.manager.register(self.trigger);
            return Err(error.to_string());
        }

        if let Err(error) = self.manager.register(next_trigger) {
            let _ = self.manager.register(self.trigger);
            let _ = self.manager.register(self.assistant);
            return Err(error.to_string());
        }
        if let Err(error) = self.manager.register(next_assistant) {
            let _ = self.manager.unregister(next_trigger);
            let _ = self.manager.register(self.trigger);
            let _ = self.manager.register(self.assistant);
            return Err(error.to_string());
        }

        self.trigger = next_trigger;
        self.assistant = next_assistant;
        if let Ok(mut ids) = self.ids.write() {
            *ids = ShortcutIds {
                trigger: next_trigger.id(),
                assistant: next_assistant.id(),
            };
        }
        Ok(())
    }
}

impl Drop for ShortcutService {
    fn drop(&mut self) {
        self.running.store(false, Ordering::Release);
        if let Some(event_thread) = self.event_thread.take() {
            let _ = event_thread.join();
        }
        let _ = self.manager.unregister(self.trigger);
        let _ = self.manager.unregister(self.assistant);
    }
}
