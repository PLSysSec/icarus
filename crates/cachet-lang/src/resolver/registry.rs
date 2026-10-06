// vim: set tw=99 ts=4 sts=4 sw=4 et:

use std::collections::HashMap;
use std::collections::hash_map::Entry;
use std::hash::Hash;

use enumset::EnumSet;
use thiserror::Error;

use crate::ast::{MaybeSpanned, Path, Span, Spanned};

use crate::resolver::error::{
    DuplicateDefError, NameKind, ResolveError, UndefinedError, WrongKindError,
};

#[derive(Clone, Debug, Error)]
pub enum LookupError {
    #[error(transparent)]
    Undefined(#[from] UndefinedError),
    #[error(transparent)]
    WrongKind(#[from] WrongKindError),
}

impl From<LookupError> for ResolveError {
    fn from(error: LookupError) -> Self {
        match error {
            LookupError::Undefined(error) => ResolveError::Undefined(error),
            LookupError::WrongKind(error) => ResolveError::WrongKind(error),
        }
    }
}

pub type LookupResult<T> = Result<(Option<T>, Option<Span>), LookupError>;

pub trait Registrable {
    fn name_kind(&self) -> NameKind;

    fn shadows(&self) -> bool;
}

#[derive(Clone)]
struct Registered<T> {
    first_defined_at: Option<Span>,
    status: RegisterStatus<T>,
}

#[derive(Clone)]
enum RegisterStatus<T> {
    RegisteredOnce(T),
    Duplicated(EnumSet<NameKind>),
}

#[derive(Clone)]
pub struct Registry<K, V> {
    inner: HashMap<K, Registered<V>>,
}

impl<K, V> Registry<K, V> {
    pub fn new() -> Self {
        Registry {
            inner: HashMap::new(),
        }
    }

    pub fn with_capacity(capacity: usize) -> Self {
        Registry {
            inner: HashMap::with_capacity(capacity),
        }
    }
}

impl<K: Eq + Hash, V> Registry<K, V> {
    pub fn register_shadowing(&mut self, key: MaybeSpanned<K>, value: V) {
        self.inner.insert(
            key.value,
            Registered {
                first_defined_at: key.span,
                status: RegisterStatus::RegisteredOnce(value),
            },
        );
    }
}

impl<K: Clone + Eq + Hash + Into<Path>, V: Registrable> Registry<K, V> {
    pub fn register(&mut self, key: MaybeSpanned<K>, value: V) -> Result<(), DuplicateDefError> {
        if value.shadows() {
            self.register_shadowing(key, value);
            return Ok(());
        }

        match self.inner.entry(key.value) {
            Entry::Vacant(vacant_entry) => {
                vacant_entry.insert(Registered {
                    first_defined_at: key.span,
                    status: RegisterStatus::RegisteredOnce(value),
                });
                Ok(())
            }
            Entry::Occupied(mut occupied_entry) => {
                let status = &mut occupied_entry.get_mut().status;
                match status {
                    RegisterStatus::RegisteredOnce(registered_value) => {
                        *status = RegisterStatus::Duplicated(
                            registered_value.name_kind() | value.name_kind(),
                        );
                    }
                    RegisterStatus::Duplicated(name_kinds) => {
                        name_kinds.insert(value.name_kind());
                    }
                }
                Err(DuplicateDefError {
                    path: occupied_entry.key().clone().into(),
                    first_defined_at: occupied_entry.get().first_defined_at,
                    redefined_at: key.span.unwrap_or(Span::Internal),
                })
            }
        }
    }
}

impl<K: Eq + Hash + Into<Path>, V: Registrable> Registry<K, V> {
    pub fn lookup(&self, key: Spanned<K>, expected: EnumSet<NameKind>) -> LookupResult<&V> {
        let registered = match self.inner.get(&key.value) {
            Some(registered) => registered,
            None => {
                return Err(LookupError::Undefined(UndefinedError {
                    path: key.map(Into::into),
                    expected,
                }));
            }
        };

        let (value, found) = match &registered.status {
            RegisterStatus::RegisteredOnce(value) => {
                let name_kind = value.name_kind();
                (Some(value), name_kind.into())
            }
            RegisterStatus::Duplicated(found) => (None, *found),
        };

        if found.is_disjoint(expected) {
            Err(LookupError::WrongKind(WrongKindError {
                path: key.map(Into::into),
                expected,
                found,
                defined_at: registered.first_defined_at,
            }))
        } else {
            Ok((value, registered.first_defined_at))
        }
    }
}
