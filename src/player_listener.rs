use pumpkin_plugin_api::{
    Item, ItemStackExt, Server,
    common::Hand,
    events::{EventData, EventHandler, InteractAction, PlayerInteractEvent, PlayerJoinEvent},
    text::TextComponent,
};

pub struct JoinEventListener;
impl EventHandler<PlayerJoinEvent> for JoinEventListener {
    fn handle(
        &self,
        _server: Server,
        mut event: EventData<PlayerJoinEvent>,
    ) -> EventData<PlayerJoinEvent> {
        event.join_message = TextComponent::text("Hello, world!");
        event
            .player
            .send_system_message(TextComponent::text("hi cool"), false);
        event
    }
}

pub struct InteractEventListener;
impl EventHandler<PlayerInteractEvent> for InteractEventListener {
    fn handle(
        &self,
        _server: Server,
        event: EventData<PlayerInteractEvent>,
    ) -> EventData<PlayerInteractEvent> {
        let is_right_click = event.action == InteractAction::RightClickBlock
            || event.action == InteractAction::RightClickAir;

        let left_hand_item = event.player.get_item_in_hand(Hand::Left);
        let right_hand_item = event.player.get_item_in_hand(Hand::Right);

        let is_holding_diamond = [&left_hand_item, &right_hand_item].into_iter().any(|item| {
            item.as_ref()
                .is_some_and(|stack| stack.is_item(Item::Diamond))
        });

        if is_right_click && is_holding_diamond {
            // my code will go here for when a player right clicks with a diamond
        }
        event
    }
}
