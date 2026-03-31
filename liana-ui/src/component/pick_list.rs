use std::borrow::Borrow;

use crate::{theme, widget::*};

pub fn pick_list<'a, T, L, V, Message>(
    options: L,
    selected: Option<V>,
    on_selected: impl Fn(T) -> Message + 'a,
) -> PickList<'a, T, L, V, Message>
where
    T: ToString + PartialEq + Clone + 'a,
    L: Borrow<[T]> + 'a,
    V: Borrow<T> + 'a,
    Message: Clone,
{
    PickList::new(options, selected, on_selected)
        .style(theme::pick_list::primary)
        .menu_style(theme::pick_list::menu)
}
