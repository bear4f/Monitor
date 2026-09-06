use serde::{Deserialize, Deserializer};

#[derive(Default)]
pub(super) enum PatchField<T> {
    #[default]
    Missing,
    Null,
    Value(T),
}

impl<'de, T> Deserialize<'de> for PatchField<T>
where
    T: Deserialize<'de>,
{
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        Option::<T>::deserialize(deserializer).map(|value| value.map_or(Self::Null, Self::Value))
    }
}

impl<T> PatchField<T> {
    pub(super) const fn is_missing(&self) -> bool {
        matches!(self, Self::Missing)
    }
}
