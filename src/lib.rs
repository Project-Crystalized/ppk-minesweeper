use pumpkin_plugin_api::{Context, Plugin, PluginMetadata, events::*};
use tracing::*;

mod player_listener;

struct DiamondSpeedPlugin;
impl Plugin for DiamondSpeedPlugin {
    fn new() -> Self {
        DiamondSpeedPlugin
    }

    fn metadata(&self) -> PluginMetadata {
        PluginMetadata {
            name: "Hello Plugin".into(),
            version: env!("CARGO_PKG_VERSION").into(),
            authors: vec!["Bjorn".into()],
            description: "A simple example plugin".into(),
            dependencies: vec![],
            permissions: vec![],
        }
    }

    fn on_load(&self, context: Context) -> pumpkin_plugin_api::Result<()> {
        info!("Hello from the speed dia plugin!");

        context
            .register_event_handler(
                player_listener::JoinEventListener,
                EventPriority::Normal,
                false,
            )
            .unwrap();
        context
            .register_event_handler(
                player_listener::InteractEventListener,
                EventPriority::Normal,
                false,
            )
            .unwrap();
        Ok(())
    }

    fn on_unload(&self, _context: Context) -> pumpkin_plugin_api::Result<()> {
        info!("Example plugin speed dia. Goodbye!");
        Ok(())
    }
}

pumpkin_plugin_api::register_plugin!(DiamondSpeedPlugin);
