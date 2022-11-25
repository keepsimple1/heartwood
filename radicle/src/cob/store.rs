//! Generic COB storage.
#![allow(clippy::large_enum_variant)]
use std::marker::PhantomData;

use crate::cob;
use crate::cob::common::Author;
use crate::cob::CollaborativeObject;
use crate::cob::{Contents, Create, History, HistoryType, ObjectId, TypeName, Update};
use crate::crypto::PublicKey;
use crate::git;
use crate::identity::project;
use crate::prelude::*;
use crate::storage::git as storage;

/// A type that can be materialized from an event history.
/// All collaborative objects implement this trait.
pub trait FromHistory: Sized {
    /// The object type name.
    fn type_name() -> &'static TypeName;
    /// Create an object from a history.
    fn from_history(history: &History) -> Result<Self, Error>;
}

/// Store error.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("create error: {0}")]
    Create(#[from] cob::error::Create),
    #[error("update error: {0}")]
    Update(#[from] cob::error::Update),
    #[error("retrieve error: {0}")]
    Retrieve(#[from] cob::error::Retrieve),
    #[error(transparent)]
    Identity(#[from] project::IdentityError),
    #[error("object `{1}`of type `{0}` was not found")]
    NotFound(TypeName, ObjectId),
}

/// Storage for collaborative objects of a specific type `T` in a single project.
pub struct Store<'a, T> {
    whoami: PublicKey,
    project: project::Identity<git::Oid>,
    raw: &'a storage::Repository,
    witness: PhantomData<T>,
}

impl<'a, T> AsRef<storage::Repository> for Store<'a, T> {
    fn as_ref(&self) -> &storage::Repository {
        self.raw
    }
}

impl<'a, T> Store<'a, T> {
    /// Open a new generic store.
    pub fn open(whoami: PublicKey, store: &'a storage::Repository) -> Result<Self, Error> {
        let project = project::Identity::load(&whoami, store)?;

        Ok(Self {
            project,
            whoami,
            raw: store,
            witness: PhantomData,
        })
    }
}

impl<'a, T> Store<'a, T> {
    /// Get this store's author.
    pub fn author(&self) -> Author {
        Author::new(self.whoami)
    }

    /// Get the public key associated with this store.
    pub fn public_key(&self) -> &PublicKey {
        &self.whoami
    }
}

impl<'a, T: FromHistory> Store<'a, T> {
    /// Update an object.
    pub fn update<G: Signer>(
        &self,
        object_id: ObjectId,
        message: &'static str,
        changes: Contents,
        signer: &G,
    ) -> Result<CollaborativeObject, cob::error::Update> {
        cob::update(
            self.raw,
            signer,
            &self.project,
            Update {
                author: Some(cob::Author::from(*signer.public_key())),
                object_id,
                history_type: HistoryType::default(),
                typename: T::type_name().clone(),
                message: message.to_owned(),
                changes,
            },
        )
    }

    /// Create an object.
    pub fn create<G: Signer>(
        &self,
        message: &'static str,
        contents: Contents,
        signer: &G,
    ) -> Result<CollaborativeObject, cob::error::Create> {
        cob::create(
            self.raw,
            signer,
            &self.project,
            Create {
                author: Some(cob::Author::from(*signer.public_key())),
                history_type: HistoryType::default(),
                typename: T::type_name().clone(),
                message: message.to_owned(),
                contents,
            },
        )
    }

    /// Get an object.
    pub fn get(&self, id: &ObjectId) -> Result<Option<T>, Error> {
        let cob = cob::get(self.raw, T::type_name(), id)?;

        if let Some(cob) = cob {
            let history = cob.history();
            let obj = T::from_history(history)?;

            Ok(Some(obj))
        } else {
            Ok(None)
        }
    }

    /// List objects.
    pub fn list(&self) -> Result<Vec<(ObjectId, T)>, Error> {
        let raw = cob::list(self.raw, T::type_name())?;

        raw.into_iter()
            .map(|o| {
                let obj = T::from_history(o.history())?;
                Ok::<_, Error>((*o.id(), obj))
            })
            .collect()
    }
}