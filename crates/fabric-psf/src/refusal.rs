//! The closed refusal codes of section 10.

use std::fmt;

/// Why a reader refused a manifest; closed in v1 (a new code is a v2 change).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Code {
    /// Over a limit of section 7.
    TooLarge,
    /// Not UTF-8, or not one JSON object.
    NotJson,
    /// Parsed, but not the canonical bytes of what was parsed.
    NotCanonical,
    /// `psf` is not `"1"`.
    UnsupportedVersion,
    /// A required key is absent, `null` or of the wrong JSON type.
    MissingField,
    /// `requires` names an extension this reader does not implement.
    UnknownRequirement,
    /// An id, digest, commit, name or integer string is not in its text form.
    BadId,
    /// A path breaks the path rules.
    BadPath,
    /// A closed-set value is outside its set.
    BadEnum,
    /// Two entries share a name or path, or a sorted list is out of order.
    Duplicate,
    /// `secrets.policy` is not `never_inside`.
    SecretPolicy,
    /// A string has a known token shape.
    CredentialLike,
    /// The fidelity is not the one the attachments and package give.
    FidelityMismatch,
    /// The package does not belong to this job or revision.
    PackageMismatch,
}

impl Code {
    /// Every code, in the order a reader checks them.
    pub const ALL: [Code; 14] = [
        Code::TooLarge,
        Code::NotJson,
        Code::NotCanonical,
        Code::UnsupportedVersion,
        Code::MissingField,
        Code::UnknownRequirement,
        Code::BadId,
        Code::BadPath,
        Code::BadEnum,
        Code::Duplicate,
        Code::SecretPolicy,
        Code::CredentialLike,
        Code::FidelityMismatch,
        Code::PackageMismatch,
    ];

    /// The code as the format document and the fixtures spell it.
    pub fn as_str(self) -> &'static str {
        match self {
            Code::TooLarge => "too_large",
            Code::NotJson => "not_json",
            Code::NotCanonical => "not_canonical",
            Code::UnsupportedVersion => "unsupported_version",
            Code::MissingField => "missing_field",
            Code::UnknownRequirement => "unknown_requirement",
            Code::BadId => "bad_id",
            Code::BadPath => "bad_path",
            Code::BadEnum => "bad_enum",
            Code::Duplicate => "duplicate",
            Code::SecretPolicy => "secret_policy",
            Code::CredentialLike => "credential_like",
            Code::FidelityMismatch => "fidelity_mismatch",
            Code::PackageMismatch => "package_mismatch",
        }
    }
}

impl fmt::Display for Code {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// A refused manifest: the first rule it breaks and where.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Refusal {
    /// The rule.
    pub code: Code,
    /// The dotted path of the value, `[i]` for an array index; empty for the whole document.
    pub field: String,
}

impl Refusal {
    pub(crate) fn new(code: Code, field: impl Into<String>) -> Self {
        Refusal {
            code,
            field: field.into(),
        }
    }
}

impl fmt::Display for Refusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.field.is_empty() {
            write!(f, "{}", self.code)
        } else {
            write!(f, "{} {}", self.code, self.field)
        }
    }
}

impl std::error::Error for Refusal {}

/// Why a reader refused a container (section 10, container level).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ContainerCode {
    /// The directory has no `manifest.json`.
    ManifestMissing,
    /// A blob does not hash to its name or has another length than declared.
    BlobMismatch,
    /// A file the manifest does not name.
    BlobUnlisted,
    /// The manifest itself was refused; the refusal's field says where.
    Manifest(Code),
    /// The directory could not be read; a file system failure, not a rule.
    Unreadable,
}

impl ContainerCode {
    /// The code as the format document and the fixtures spell it.
    pub fn as_str(self) -> &'static str {
        match self {
            ContainerCode::ManifestMissing => "manifest_missing",
            ContainerCode::BlobMismatch => "blob_mismatch",
            ContainerCode::BlobUnlisted => "blob_unlisted",
            ContainerCode::Manifest(code) => code.as_str(),
            ContainerCode::Unreadable => "unreadable",
        }
    }
}

impl fmt::Display for ContainerCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// A refused container: the rule, the file inside the directory, and for a
/// refused manifest the field.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContainerRefusal {
    /// The rule.
    pub code: ContainerCode,
    /// The path inside the container, such as `blobs/<digest>` or `manifest.json`.
    pub path: String,
    /// For [`ContainerCode::Manifest`], the manifest field; empty otherwise.
    pub field: String,
}

impl fmt::Display for ContainerRefusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} {}", self.code, self.path)?;
        if !self.field.is_empty() {
            write!(f, " {}", self.field)?;
        }
        Ok(())
    }
}

impl std::error::Error for ContainerRefusal {}
