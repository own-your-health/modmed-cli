//! `modmed`: a patient's health records from a ModMed practice through
//! ModMed's Certified (ONC) FHIR API, as the `health-records/v1` and
//! `documents/v1` profiles (medical-common). `docs/api.md` has the API, its
//! sources and its traps.

pub mod cli;
pub mod commands;
pub mod config;
pub mod credentials;
pub mod export;
pub mod fsutil;
pub mod modmed;
pub mod orgs;
pub mod session;
