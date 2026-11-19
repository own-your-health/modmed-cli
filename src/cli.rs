//! The command tree.

use clap::{Args, Parser, Subcommand};
use clap_complete::Shell;
use pk_cli_core::CommonArgs;
use pk_cli_secrets::SecretSourceArgs;
use pk_cli_selfupdate::SelfUpdateArgs;

/// Pull your own health records from a ModMed practice (EMA, gGastro and
/// ModMed's other EHRs) through ModMed's Certified FHIR API. Unofficial;
/// not affiliated with or endorsed by ModMed (Modernizing Medicine, Inc.).
#[derive(Parser, Debug)]
#[command(name = "modmed", version, about, long_about = None)]
pub struct Cli {
    #[command(flatten)]
    pub common: CommonArgs,

    /// Config file location (default ~/.config/modmed/config.json).
    #[arg(long, global = true, value_name = "PATH", env = "MODMED_CONFIG")]
    pub config: Option<std::path::PathBuf>,

    /// The login to use: one patient portal account, added with `modmed
    /// profiles add`. Required by `auth login|status|logout`, `patients`,
    /// `records`, `documents` and `api`, and refused by every other command
    /// (`profiles remove` takes the name as its argument); `MODMED_PROFILE`
    /// when not given. There is no default.
    #[arg(long, global = true, value_name = "NAME")]
    pub profile: Option<String>,

    #[command(subcommand)]
    pub command: Command,
}

#[derive(Subcommand, Debug)]
pub enum Command {
    /// The practices ModMed's endpoint directories list (org-list/v1),
    /// downloaded weekly.
    #[command(subcommand)]
    Orgs(OrgsCmd),
    /// The logins modmed keeps, one per patient portal account
    /// (profile-list/v1).
    #[command(subcommand)]
    Profiles(ProfilesCmd),
    /// The app's client registration with ModMed, signing in as a patient,
    /// and the stored sign-ins.
    #[command(subcommand)]
    Auth(AuthCmd),
    /// The patients signed in (patient-list/v1).
    #[command(subcommand)]
    Patients(PatientsCmd),
    /// A patient's records as a records-stream/v1 NDJSON stream.
    #[command(subcommand)]
    Records(RecordsCmd),
    /// A patient's documents: the DocumentReference attachments
    /// (documents/v1).
    #[command(subcommand)]
    Documents(DocumentsCmd),
    /// One FHIR read as the signed-in patient: `modmed api GET
    /// Condition?patient=<id>`.
    Api(ApiArgs),
    /// Non-secret settings: the redirect, a test server, the directory.
    #[command(subcommand)]
    Config(ConfigCmd),
    /// Update to the latest release from GitHub.
    SelfUpdate(SelfUpdateArgs),
    /// Print a shell completion script.
    Completions { shell: Shell },
    /// Machine-readable capability discovery (cli-info/v1).
    Info,
}

impl Command {
    /// Whether the command runs as a profile, so `--profile` means
    /// something to it.
    pub fn uses_profile(&self) -> bool {
        match self {
            Command::Auth(cmd) => !matches!(cmd, AuthCmd::SetCredential(_)),
            Command::Patients(_)
            | Command::Records(_)
            | Command::Documents(_)
            | Command::Api(_) => true,
            Command::Orgs(_)
            | Command::Profiles(_)
            | Command::Config(_)
            | Command::SelfUpdate(_)
            | Command::Completions { .. }
            | Command::Info => false,
        }
    }
}

#[derive(Subcommand, Debug)]
pub enum OrgsCmd {
    /// Download ModMed's directories if the cached copy is a week old.
    Refresh {
        /// Download even if the cached copy is less than a week old.
        #[arg(long)]
        force: bool,
    },
    /// Every practice in the directories, by name.
    #[command(visible_alias = "ls")]
    List(OrgListArgs),
    /// The practices whose name, id, subdomain, city, state or NPI contains
    /// every word given.
    Search {
        /// Words to match, any case: `modmed orgs search dermatology springfield`.
        #[arg(required = true, num_args = 1.., value_name = "WORD")]
        words: Vec<String>,
        #[command(flatten)]
        list: OrgListArgs,
    },
}

#[derive(Args, Debug, Clone, Default)]
pub struct OrgListArgs {
    /// At most N practices.
    #[arg(long, value_name = "N")]
    pub limit: Option<usize>,
    /// Only practices in this US state (two letters).
    #[arg(long, value_name = "ST")]
    pub state: Option<String>,
}

#[derive(Subcommand, Debug)]
pub enum ProfilesCmd {
    /// The configured profiles (profile-list/v1).
    #[command(visible_alias = "ls")]
    List,
    /// Add a profile: one patient portal account (`adult1`). Then `modmed
    /// auth login --profile <NAME> --org <ORG>` signs its patients in.
    Add {
        /// 1 to 32 lowercase letters, digits, `-` or `_`.
        name: String,
    },
    /// Remove a profile with its sign-ins and its browser profile. Asks
    /// first; --force to run without asking.
    Remove {
        name: String,
        /// Remove without asking.
        #[arg(long)]
        force: bool,
    },
}

#[derive(Subcommand, Debug)]
pub enum AuthCmd {
    /// Store the client id and secret ModMed issued for this household's
    /// app, in modmed's vault (shared by every profile and practice). On a
    /// terminal modmed asks for each without echoing it; with --stdin or
    /// --from-env it reads `<id>:<secret>`, or the id and the secret on two
    /// lines. Never on the command line.
    SetCredential(SetCredentialArgs),
    /// Sign in as a patient in Chrome, in the profile's own browser profile
    /// (the practice's patient portal sign-in, ModMed's patient picker and
    /// consent, PKCE, a redirect to this machine). One sign-in per practice
    /// and patient.
    Login(LoginArgs),
    /// What is stored and whether it can be used (auth-status/v1).
    Status,
    /// Remove the profile's stored sign-ins (all, or those --org and
    /// --patient select). With --forget, once no other profile holds a
    /// sign-in, also what every profile shares: the client credentials and
    /// the vault's key.
    Logout(LogoutArgs),
}

#[derive(Args, Debug)]
pub struct SetCredentialArgs {
    #[command(flatten)]
    pub source: SecretSourceArgs,
    /// The client for a test server on this machine (`config set
    /// test_fhir_base`) instead of ModMed's, so ModMed's secret never goes
    /// to one.
    #[arg(long)]
    pub test: bool,
    /// Replace a client already stored for this environment.
    #[arg(long)]
    pub overwrite: bool,
}

/// `auth login --timeout`'s default, in minutes.
pub const LOGIN_TIMEOUT: u64 = 10;

#[derive(Args, Debug)]
pub struct LoginArgs {
    /// The practice: an id from `modmed orgs search` (`<practice>.<zone>`),
    /// its subdomain, NPI or name, or `test` for a test server on this
    /// machine.
    #[arg(long, value_name = "ORG")]
    pub org: String,
    /// A name for this patient in this household (`adult1`, `child1`), to
    /// pick them with --patient later.
    #[arg(long, value_name = "WHO")]
    pub label: Option<String>,
    /// Print the sign-in URL instead of opening Chrome.
    #[arg(long)]
    pub no_browser: bool,
    /// How long to wait for the sign-in to finish, in minutes.
    #[arg(long, value_name = "MIN", default_value_t = LOGIN_TIMEOUT)]
    pub timeout: u64,
}

#[derive(Args, Debug)]
pub struct LogoutArgs {
    #[command(flatten)]
    pub who: PatientArgs,
    /// Also remove what every profile shares, unless another profile still
    /// holds a sign-in: the client id and secret of both environments, and
    /// the vault's key once no entry is left. The config and its profiles
    /// stay.
    #[arg(long)]
    pub forget: bool,
}

#[derive(Subcommand, Debug)]
pub enum PatientsCmd {
    /// The patients signed in, and how they were reached (self or proxy).
    #[command(visible_alias = "ls")]
    List {
        /// Also read each stored sign-in to report whether it is usable
        /// (reads modmed's vault).
        #[arg(long)]
        status: bool,
        /// Only patients at this practice: its id or name as signed in.
        #[arg(long, value_name = "ORG")]
        org: Option<String>,
    },
}

/// Which signed-in patient a command reads.
#[derive(Args, Debug, Clone, Default)]
pub struct PatientArgs {
    /// The patient: a label from `auth login --label` or the FHIR id from
    /// `patients list`. Needed when more than one patient is signed in.
    #[arg(long, value_name = "ID")]
    pub patient: Option<String>,
    /// Only patients at this practice: its id (`patients list` shows it) or
    /// name as signed in.
    #[arg(long, value_name = "ORG")]
    pub org: Option<String>,
}

#[derive(Subcommand, Debug)]
pub enum RecordsCmd {
    /// Every record the practice serves for the patient (conditions,
    /// medications, results, notes and their files, visits, immunizations,
    /// care plans and the rest), with the practitioners, organizations,
    /// locations and medications they name, as records-stream/v1 (what
    /// `medrec import --from modmed` reads).
    Export(ExportArgs),
}

#[derive(Args, Debug)]
pub struct ExportArgs {
    #[command(flatten)]
    pub who: PatientArgs,
    /// Only resources dated on or after this day (YYYY-MM-DD), for the
    /// types that have a date to search by; the rest come whole.
    #[arg(long, value_name = "YYYY-MM-DD")]
    pub since: Option<String>,
    /// Write the stream to stdout.
    #[arg(long, conflicts_with = "output")]
    pub ndjson: bool,
    /// Write the stream to DIR/records.ndjson instead (test servers only:
    /// real records go straight into medrec).
    #[arg(short, long, value_name = "DIR")]
    pub output: Option<std::path::PathBuf>,
}

#[derive(Subcommand, Debug)]
pub enum DocumentsCmd {
    /// The patient's documents, newest first (document-list/v1).
    #[command(visible_alias = "ls")]
    List {
        #[command(flatten)]
        who: PatientArgs,
        #[command(flatten)]
        range: pk_cli_documents::RangeArgs,
    },
    /// Save one document (`documents download <ID> -o <PATH>`) or every one
    /// (`--all -o <DIR>`).
    Download {
        /// The document id from `documents list`.
        #[arg(required_unless_present = "all", conflicts_with = "all")]
        id: Option<String>,
        /// Every document in the list.
        #[arg(long)]
        all: bool,
        /// A file, or a directory to save into.
        #[arg(short, long, value_name = "PATH")]
        output: std::path::PathBuf,
        #[command(flatten)]
        who: PatientArgs,
        #[command(flatten)]
        range: pk_cli_documents::RangeArgs,
    },
}

#[derive(Args, Debug)]
pub struct ApiArgs {
    /// GET only: modmed reads and never writes.
    pub method: String,
    /// A path under the practice's FHIR base (`Condition?patient=<id>`,
    /// `Observation/<id>`), or a URL on that server.
    pub path: String,
    #[command(flatten)]
    pub who: PatientArgs,
}

#[derive(Subcommand, Debug)]
pub enum ConfigCmd {
    /// Print the config file path.
    Path,
    /// Show the effective configuration.
    Show,
    /// Set a key: redirect_port, redirect_page, test_fhir_base,
    /// directory_url.
    Set { key: String, value: String },
    /// Remove a key.
    Unset { key: String },
}
