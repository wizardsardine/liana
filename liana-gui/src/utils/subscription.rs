use iced::advanced::subscription::{from_recipe, EventStream, Hasher, Recipe};
use iced::futures::stream::BoxStream;
use iced::futures::Stream;
use iced::Subscription;

use std::hash::Hash;

/// Replacement for `Subscription::run_with_id` which was removed in iced 0.14.
///
/// Creates a [`Subscription`] that will asynchronously run the given [`Stream`],
/// using `id` to uniquely identify the subscription.
pub fn run_with_id<I, S, T>(id: I, stream: S) -> Subscription<T>
where
    I: Hash + 'static,
    S: Stream<Item = T> + Send + 'static,
    T: 'static + Send,
{
    from_recipe(IdRunner { id, stream })
}

struct IdRunner<I, S> {
    id: I,
    stream: S,
}

impl<I, S> Recipe for IdRunner<I, S>
where
    I: Hash + 'static,
    S: Stream + Send + 'static,
    S::Item: 'static + Send,
{
    type Output = S::Item;

    fn hash(&self, state: &mut Hasher) {
        self.id.hash(state);
        std::any::TypeId::of::<S>().hash(state);
    }

    fn stream(self: Box<Self>, _input: EventStream) -> BoxStream<'static, Self::Output> {
        Box::pin(self.stream)
    }
}
