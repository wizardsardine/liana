use crate::daemon::model::Coin;

#[derive(Default)]
pub struct Cache {
    pub blockheight: i32,
    pub coins: Vec<Coin>,
}
