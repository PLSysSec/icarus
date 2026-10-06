// vim: set tw=99 ts=4 sts=4 sw=4 et:

use std::borrow::Borrow;
use std::fmt::{self, Debug, Display};
use std::ops::Deref;
use std::vec::IntoIter as VecIntoIter;

use derive_more::Display;
use internment::Intern;

use cachet_util::{deref_from, fmt_join};

// TODO(spinda): Expand derives.
#[derive(Clone, Copy, Display, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct Ident(Intern<String>);

impl Ident {
    pub fn nest(self, ident: Ident) -> Path {
        Path::new(Some(Path::from(self)), ident)
    }

    pub fn from_display(name: impl ToString) -> Self {
        Ident(name.to_string().into())
    }
}

impl<T: ?Sized> AsRef<T> for Ident
where
    String: AsRef<T>,
{
    fn as_ref(&self) -> &T {
        (**self).as_ref()
    }
}

impl<T: ?Sized> Borrow<T> for Ident
where
    String: Borrow<T>,
{
    fn borrow(&self) -> &T {
        (**self).borrow()
    }
}

impl Deref for Ident {
    type Target = String;

    fn deref(&self) -> &Self::Target {
        &*self.0
    }
}

impl Debug for Ident {
    fn fmt(&self, f: &mut fmt::Formatter) -> Result<(), fmt::Error> {
        Debug::fmt(&**self, f)
    }
}

impl From<String> for Ident {
    fn from(s: String) -> Self {
        Ident(s.into())
    }
}

impl From<&String> for Ident {
    fn from(s: &String) -> Self {
        String::from(s).into()
    }
}

impl From<&str> for Ident {
    fn from(s: &str) -> Self {
        String::from(s).into()
    }
}

impl From<&mut str> for Ident {
    fn from(s: &mut str) -> Self {
        String::from(s).into()
    }
}

impl From<Ident> for String {
    fn from(ident: Ident) -> Self {
        (*ident).clone()
    }
}

#[derive(Clone, Copy, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct Path(Intern<PathNode>);

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
struct PathNode {
    parent: Option<Path>,
    ident: Ident,
}

impl Path {
    pub fn new<T: Into<Ident>>(parent: Option<Path>, ident: T) -> Self {
        Path(Intern::new(PathNode {
            parent,
            ident: ident.into(),
        }))
    }

    pub fn from_ident<T: Into<Ident>>(ident: T) -> Self {
        Path::new(None, ident.into())
    }

    pub fn parent(self) -> Option<Path> {
        self.0.parent
    }

    pub fn has_parent(self) -> bool {
        self.parent().is_some()
    }

    pub fn ident(self) -> Ident {
        self.0.ident
    }

    pub fn nest<T: Into<Ident>>(self, ident: T) -> Path {
        Path::new(Some(self), ident.into())
    }

    pub fn push<T: Into<Ident>>(&mut self, ident: T) {
        *self = self.nest(ident.into());
    }

    pub fn len(mut self) -> usize {
        let mut len = 0;
        while let Some(parent) = self.parent() {
            len += 1;
            self = parent;
        }
        len + 1
    }

    pub fn segments(mut self) -> Vec<Ident> {
        let mut segments = Vec::with_capacity(self.len());
        loop {
            segments.push(self.ident());
            if let Some(parent) = self.parent() {
                self = parent;
            } else {
                break;
            }
        }
        segments.reverse();
        segments
    }
}

impl Debug for Path {
    fn fmt(&self, f: &mut fmt::Formatter) -> Result<(), fmt::Error> {
        Debug::fmt(&format!("{}", self), f)
    }
}

impl Display for Path {
    fn fmt(&self, f: &mut fmt::Formatter) -> Result<(), fmt::Error> {
        fmt_join(f, "::", self.into_iter())
    }
}

impl From<Ident> for Path {
    fn from(ident: Ident) -> Self {
        Path::from_ident(ident)
    }
}

deref_from!(&Ident => Path);

impl Extend<Ident> for Path {
    fn extend<T>(&mut self, iter: T)
    where
        T: IntoIterator<Item = Ident>,
    {
        for ident in iter.into_iter() {
            self.push(ident);
        }
    }
}

impl<'a> Extend<&'a Ident> for Path {
    fn extend<T>(&mut self, iter: T)
    where
        T: IntoIterator<Item = &'a Ident>,
    {
        self.extend(iter.into_iter().copied());
    }
}

impl FromIterator<Ident> for Path {
    fn from_iter<T>(iter: T) -> Self
    where
        T: IntoIterator<Item = Ident>,
    {
        let mut iter = iter.into_iter();
        let mut path = Path::from(iter.next().expect("Paths can't be empty"));
        path.extend(iter);
        path
    }
}

impl<'a> FromIterator<&'a Ident> for Path {
    fn from_iter<T>(iter: T) -> Self
    where
        T: IntoIterator<Item = &'a Ident>,
    {
        Path::from_iter(iter.into_iter().copied())
    }
}

impl IntoIterator for Path {
    type Item = Ident;
    type IntoIter = VecIntoIter<Ident>;

    fn into_iter(self) -> Self::IntoIter {
        self.segments().into_iter()
    }
}

impl IntoIterator for &Path {
    type Item = Ident;
    type IntoIter = <Path as IntoIterator>::IntoIter;

    fn into_iter(self) -> Self::IntoIter {
        (*self).into_iter()
    }
}
