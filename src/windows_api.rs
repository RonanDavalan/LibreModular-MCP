//! Les appels à l'API de Windows dont le Hub a besoin : fichiers privés à l'utilisateur, propriétaire
//! et liste de contrôle d'accès d'un fichier ouvert, arbre de processus borné.
//!
//! Pourquoi ce module existe
//! La bibliothèque standard ne sait ni créer un fichier avec une liste de contrôle d'accès, ni la lire,
//! ni borner un arbre de processus. Ces appels sont du code non sûr : ils vivent ici et nulle part
//! ailleurs, chaque fonction exposée est sûre et rend un résultat d'E/S. `security.rs` reste le seul
//! module qui décide ce qui est privé.

#![allow(unsafe_code)]

use std::ffi::OsStr;
use std::fs::File;
use std::io;
use std::os::windows::ffi::OsStrExt;
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
use std::path::Path;
use std::process::Child;
use std::ptr::{null, null_mut};
use std::sync::OnceLock;

use windows_sys::Win32::Foundation::{
    ERROR_SUCCESS, GENERIC_READ, GENERIC_WRITE, GetLastError, HANDLE, INVALID_HANDLE_VALUE,
    LocalFree,
};
use windows_sys::Win32::Security::Authorization::{
    ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW, GetSecurityInfo,
    SDDL_REVISION_1, SE_FILE_OBJECT,
};
use windows_sys::Win32::Security::{
    ACL, ACL_SIZE_INFORMATION, AclSizeInformation, CreateWellKnownSid, DACL_SECURITY_INFORMATION,
    EqualSid, GetAce, GetAclInformation, GetLengthSid, GetTokenInformation,
    OWNER_SECURITY_INFORMATION, PSECURITY_DESCRIPTOR, PSID, SECURITY_ATTRIBUTES, TOKEN_OWNER,
    TOKEN_QUERY, TOKEN_USER, TokenOwner, TokenUser, WinBuiltinAdministratorsSid,
    WinCreatorOwnerRightsSid, WinCreatorOwnerSid, WinLocalSystemSid,
};
use windows_sys::Win32::Storage::FileSystem::{
    CREATE_NEW, CreateDirectoryW, CreateFileW, FILE_APPEND_DATA, FILE_FLAG_BACKUP_SEMANTICS,
    FILE_FLAG_OPEN_REPARSE_POINT, FILE_READ_ATTRIBUTES, FILE_SHARE_DELETE, FILE_SHARE_READ,
    FILE_SHARE_WRITE, OPEN_ALWAYS, OPEN_EXISTING, READ_CONTROL,
};
use windows_sys::Win32::System::Diagnostics::Debug::{
    SEM_FAILCRITICALERRORS, SEM_NOGPFAULTERRORBOX, SetErrorMode,
};
use windows_sys::Win32::System::JobObjects::{
    AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
    JOB_OBJECT_LIMIT_PROCESS_MEMORY, JOB_OBJECT_LIMIT_PROCESS_TIME,
    JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectExtendedLimitInformation,
    SetInformationJobObject, TerminateJobObject,
};
use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

/// Taille de la mémoire tampon d'un identifiant de sécurité (`SECURITY_MAX_SID_SIZE`).
const MAX_SID_BYTES: usize = 68;

/// Le propriétaire d'un fichier, rangé dans l'une des trois classes que le Hub distingue.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Owner {
    /// Le compte de l'utilisateur qui exécute le Hub.
    CurrentUser,
    /// `SYSTEM` ou le groupe des administrateurs : l'équivalent de `root`.
    Administrators,
    /// Tout autre compte.
    Other,
}

fn last_error() -> io::Error {
    // SAFETY: GetLastError n'a aucun préalable.
    io::Error::from_raw_os_error(unsafe { GetLastError() } as i32)
}

fn wide(path: &Path) -> Vec<u16> {
    OsStr::new(path)
        .encode_wide()
        .chain(std::iter::once(0))
        .collect()
}

/// Un identifiant de sécurité copié hors de la mémoire du système.
#[derive(Clone)]
struct Sid(Vec<u8>);

impl Sid {
    fn from_raw(sid: PSID) -> io::Result<Sid> {
        if sid.is_null() {
            return Err(io::Error::other("null security identifier"));
        }
        // SAFETY: `sid` est un identifiant valide rendu par le système ; sa longueur est lue par lui.
        let length = unsafe { GetLengthSid(sid) } as usize;
        // SAFETY: l'identifiant occupe exactement `length` octets.
        let bytes = unsafe { std::slice::from_raw_parts(sid.cast::<u8>(), length) };
        Ok(Sid(bytes.to_vec()))
    }

    fn well_known(kind: i32) -> io::Result<Sid> {
        let mut buffer = vec![0_u8; MAX_SID_BYTES];
        let mut size = u32::try_from(MAX_SID_BYTES).unwrap_or(u32::MAX);
        // SAFETY: la mémoire tampon a la taille annoncée ; aucun domaine n'est demandé.
        let done =
            unsafe { CreateWellKnownSid(kind, null_mut(), buffer.as_mut_ptr().cast(), &mut size) };
        if done == 0 {
            return Err(last_error());
        }
        buffer.truncate(size as usize);
        Ok(Sid(buffer))
    }

    fn ptr(&self) -> PSID {
        self.0.as_ptr().cast_mut().cast()
    }

    fn matches(&self, other: PSID) -> bool {
        // SAFETY: les deux identifiants sont valides.
        !other.is_null() && unsafe { EqualSid(self.ptr(), other) } != 0
    }

    fn text(&self) -> io::Result<String> {
        let mut out: *mut u16 = null_mut();
        // SAFETY: l'identifiant est valide ; la chaîne rendue est libérée plus bas.
        if unsafe { ConvertSidToStringSidW(self.ptr(), &mut out) } == 0 {
            return Err(last_error());
        }
        let mut length = 0;
        // SAFETY: la chaîne est terminée par un zéro.
        while unsafe { *out.add(length) } != 0 {
            length += 1;
        }
        // SAFETY: `length` unités sont lisibles.
        let text = String::from_utf16_lossy(unsafe { std::slice::from_raw_parts(out, length) });
        // SAFETY: la mémoire vient du système et n'est plus utilisée.
        unsafe { LocalFree(out.cast()) };
        Ok(text)
    }
}

struct Principals {
    user: Sid,
    /// Le propriétaire que reçoit tout objet créé par ce processus : l'utilisateur, ou le groupe des
    /// administrateurs quand le compte en est membre et que le jeton est complet.
    created_by_me: Sid,
    administrators: Sid,
    system: Sid,
    /// `CREATOR OWNER` et `OWNER RIGHTS` : des identités qui désignent le propriétaire du fichier, déjà contrôlé.
    owner_aliases: [Sid; 2],
}

impl Principals {
    fn classify(&self, sid: PSID) -> Owner {
        if self.user.matches(sid) || self.created_by_me.matches(sid) {
            Owner::CurrentUser
        } else if self.administrators.matches(sid)
            || self.system.matches(sid)
            || self.owner_aliases.iter().any(|alias| alias.matches(sid))
        {
            Owner::Administrators
        } else {
            Owner::Other
        }
    }
}

fn current_user_sid() -> io::Result<Sid> {
    token_sid(false)
}

fn default_owner_sid() -> io::Result<Sid> {
    token_sid(true)
}

/// L'identifiant de l'utilisateur du jeton du processus, ou de son propriétaire par défaut.
fn token_sid(owner: bool) -> io::Result<Sid> {
    let mut handle: HANDLE = null_mut();
    // SAFETY: le processus courant est toujours valide.
    let opened = unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut handle) };
    if opened == 0 {
        return Err(last_error());
    }
    // SAFETY: OpenProcessToken a rendu une poignée que l'on possède.
    let process_handle = unsafe { OwnedHandle::from_raw_handle(handle) };
    let class = if owner { TokenOwner } else { TokenUser };
    let mut needed = 0_u32;
    // SAFETY: premier appel pour connaître la taille ; le résultat d'échec attendu est ignoré.
    unsafe {
        GetTokenInformation(
            process_handle.as_raw_handle(),
            class,
            null_mut(),
            0,
            &mut needed,
        );
    }
    if needed == 0 {
        return Err(last_error());
    }
    let mut buffer = vec![0_u64; (needed as usize).div_ceil(8)];
    // SAFETY: la mémoire tampon fait au moins `needed` octets et est alignée sur 8.
    let done = unsafe {
        GetTokenInformation(
            process_handle.as_raw_handle(),
            class,
            buffer.as_mut_ptr().cast(),
            needed,
            &mut needed,
        )
    };
    if done == 0 {
        return Err(last_error());
    }
    let sid = if owner {
        // SAFETY: la mémoire tampon contient un TOKEN_OWNER dont le SID pointe dans la même mémoire.
        unsafe { (*buffer.as_ptr().cast::<TOKEN_OWNER>()).Owner }
    } else {
        // SAFETY: la mémoire tampon contient un TOKEN_USER dont le SID pointe dans la même mémoire.
        unsafe { (*buffer.as_ptr().cast::<TOKEN_USER>()).User.Sid }
    };
    Sid::from_raw(sid)
}

fn principals() -> io::Result<&'static Principals> {
    static CACHE: OnceLock<Result<Principals, String>> = OnceLock::new();
    CACHE
        .get_or_init(|| {
            let build = || -> io::Result<Principals> {
                Ok(Principals {
                    user: current_user_sid()?,
                    created_by_me: default_owner_sid()?,
                    administrators: Sid::well_known(WinBuiltinAdministratorsSid)?,
                    system: Sid::well_known(WinLocalSystemSid)?,
                    owner_aliases: [
                        Sid::well_known(WinCreatorOwnerSid)?,
                        Sid::well_known(WinCreatorOwnerRightsSid)?,
                    ],
                })
            };
            build().map_err(|error| error.to_string())
        })
        .as_ref()
        .map_err(|message| io::Error::other(message.clone()))
}

/// Ne laisse ni boîte de dialogue d'erreur ni rapport d'erreur interactif au processus.
pub fn harden() {
    // SAFETY: SetErrorMode n'a aucun préalable.
    unsafe {
        SetErrorMode(SEM_FAILCRITICALERRORS | SEM_NOGPFAULTERRORBOX);
    }
}

/// Un descripteur de sécurité qui ne donne le contrôle total qu'à l'utilisateur courant, sans
/// hériter de la liste du répertoire parent.
struct PrivateDescriptor {
    descriptor: PSECURITY_DESCRIPTOR,
}

impl PrivateDescriptor {
    fn new(inherit_to_children: bool) -> io::Result<PrivateDescriptor> {
        let user = principals()?.user.text()?;
        let flags = if inherit_to_children { "OICI" } else { "" };
        let sddl: Vec<u16> = format!("D:P(A;{flags};FA;;;{user})")
            .encode_utf16()
            .chain(std::iter::once(0))
            .collect();
        let mut descriptor: PSECURITY_DESCRIPTOR = null_mut();
        // SAFETY: la chaîne est terminée par un zéro ; le descripteur rendu est libéré à la chute.
        let done = unsafe {
            ConvertStringSecurityDescriptorToSecurityDescriptorW(
                sddl.as_ptr(),
                SDDL_REVISION_1,
                &mut descriptor,
                null_mut(),
            )
        };
        if done == 0 {
            return Err(last_error());
        }
        Ok(PrivateDescriptor { descriptor })
    }

    fn attributes(&self) -> SECURITY_ATTRIBUTES {
        SECURITY_ATTRIBUTES {
            nLength: u32::try_from(size_of::<SECURITY_ATTRIBUTES>()).unwrap_or(0),
            lpSecurityDescriptor: self.descriptor,
            bInheritHandle: 0,
        }
    }
}

impl Drop for PrivateDescriptor {
    fn drop(&mut self) {
        // SAFETY: le descripteur vient de ConvertStringSecurityDescriptorToSecurityDescriptorW.
        unsafe { LocalFree(self.descriptor.cast()) };
    }
}

/// Comment ouvrir un fichier de contrôle.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Access {
    Read,
    AppendOrCreate,
    CreateNew,
}

/// Ouvre sans suivre un point d'analyse final ; un fichier créé naît avec une liste de contrôle
/// d'accès privée à l'utilisateur. Le fichier est partageable en lecture, écriture et suppression, pour
/// que le journal tourne et que le jeton se remplace.
pub fn open_file(path: &Path, access: Access) -> io::Result<File> {
    // Lire les attributs et la liste de contrôle d'accès du fichier ouvert fait partie de tout accès.
    let (rights, disposition) = match access {
        Access::Read => (GENERIC_READ, OPEN_EXISTING),
        Access::AppendOrCreate => (FILE_APPEND_DATA, OPEN_ALWAYS),
        Access::CreateNew => (GENERIC_WRITE, CREATE_NEW),
    };
    let rights = rights | READ_CONTROL | FILE_READ_ATTRIBUTES;
    let private = if access == Access::Read {
        None
    } else {
        Some(PrivateDescriptor::new(false)?)
    };
    let attributes = private.as_ref().map(PrivateDescriptor::attributes);
    let name = wide(path);
    // SAFETY: le nom est terminé par un zéro et les attributs vivent jusqu'à la fin de l'appel.
    let handle = unsafe {
        CreateFileW(
            name.as_ptr(),
            rights,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            attributes.as_ref().map_or(null(), std::ptr::from_ref),
            disposition,
            FILE_FLAG_OPEN_REPARSE_POINT,
            null_mut(),
        )
    };
    if handle == INVALID_HANDLE_VALUE {
        return Err(last_error());
    }
    // SAFETY: CreateFileW a rendu une poignée de fichier que l'on possède.
    Ok(unsafe { File::from_raw_handle(handle) })
}

/// Crée un répertoire dont la liste de contrôle d'accès ne donne le contrôle qu'à l'utilisateur,
/// héritée par ce qu'il contiendra.
pub fn create_private_dir(path: &Path) -> io::Result<()> {
    let private = PrivateDescriptor::new(true)?;
    let attributes = private.attributes();
    let name = wide(path);
    // SAFETY: le nom est terminé par un zéro et les attributs vivent jusqu'à la fin de l'appel.
    if unsafe { CreateDirectoryW(name.as_ptr(), &attributes) } == 0 {
        return Err(last_error());
    }
    Ok(())
}

/// Ce que `GetSecurityInfo` a rendu, libéré à la chute.
struct SecurityInfo {
    descriptor: PSECURITY_DESCRIPTOR,
    owner: PSID,
    dacl: *mut ACL,
}

impl SecurityInfo {
    fn of(file: &File, parts: u32) -> io::Result<SecurityInfo> {
        let mut info = SecurityInfo {
            descriptor: null_mut(),
            owner: null_mut(),
            dacl: null_mut(),
        };
        // SAFETY: la poignée est celle d'un fichier ouvert ; les sorties sont des pointeurs locaux.
        let status = unsafe {
            GetSecurityInfo(
                file.as_raw_handle(),
                SE_FILE_OBJECT,
                parts,
                &mut info.owner,
                null_mut(),
                &mut info.dacl,
                null_mut(),
                &mut info.descriptor,
            )
        };
        if status != ERROR_SUCCESS {
            return Err(io::Error::from_raw_os_error(status as i32));
        }
        Ok(info)
    }
}

impl Drop for SecurityInfo {
    fn drop(&mut self) {
        if !self.descriptor.is_null() {
            // SAFETY: la mémoire a été allouée par GetSecurityInfo.
            unsafe { LocalFree(self.descriptor.cast()) };
        }
    }
}

/// Le propriétaire d'un fichier ouvert.
pub fn owner_of(file: &File) -> io::Result<Owner> {
    let info = SecurityInfo::of(file, OWNER_SECURITY_INFORMATION)?;
    Ok(principals()?.classify(info.owner))
}

const ACCESS_ALLOWED_TYPES: [u8; 4] = [0, 5, 9, 11];
const ACCESS_DENIED_TYPES: [u8; 4] = [1, 6, 10, 12];

/// Les droits qui permettent de modifier un fichier : écrire, ajouter, changer les attributs étendus
/// et les attributs, supprimer, changer la liste de contrôle d'accès ou le propriétaire, et les droits
/// génériques d'écriture et de contrôle total.
pub const MODIFY_RIGHTS: u32 = 0x0000_0002
    | 0x0000_0004
    | 0x0000_0010
    | 0x0000_0100
    | 0x0001_0000
    | 0x0004_0000
    | 0x0008_0000
    | 0x4000_0000
    | 0x1000_0000;

/// Tous les droits.
pub const ANY_RIGHT: u32 = u32::MAX;

/// Dit si la liste de contrôle d'accès d'un fichier ou d'un répertoire ouvert accorde l'un des
/// droits de `rights` à un compte autre que l'utilisateur, `SYSTEM` et les administrateurs. Une liste
/// absente (accès à tout le monde) ou une entrée d'un type inconnu compte comme un octroi : échec fermé.
pub fn grants_to_others(file: &File, rights: u32) -> io::Result<bool> {
    let info = SecurityInfo::of(file, DACL_SECURITY_INFORMATION)?;
    if info.dacl.is_null() {
        return Ok(true);
    }
    let principals = principals()?;
    let mut size = ACL_SIZE_INFORMATION {
        AceCount: 0,
        AclBytesInUse: 0,
        AclBytesFree: 0,
    };
    // SAFETY: la liste est valide et la structure a la taille annoncée.
    let done = unsafe {
        GetAclInformation(
            info.dacl,
            std::ptr::from_mut(&mut size).cast(),
            u32::try_from(size_of::<ACL_SIZE_INFORMATION>()).unwrap_or(0),
            AclSizeInformation,
        )
    };
    if done == 0 {
        return Err(last_error());
    }
    for index in 0..size.AceCount {
        let mut entry: *mut core::ffi::c_void = null_mut();
        // SAFETY: l'indice est inférieur au nombre d'entrées.
        if unsafe { GetAce(info.dacl, index, &mut entry) } == 0 {
            return Err(last_error());
        }
        // SAFETY: une entrée commence par un en-tête dont le premier octet est le type.
        let kind = unsafe { *entry.cast::<u8>() };
        if ACCESS_DENIED_TYPES.contains(&kind) {
            continue;
        }
        if !ACCESS_ALLOWED_TYPES.contains(&kind) || kind != 0 {
            return Ok(true);
        }
        // SAFETY: l'en-tête fait 4 octets, puis vient le masque de 4 octets.
        let mask = unsafe { entry.cast::<u8>().add(4).cast::<u32>().read_unaligned() };
        if mask & rights == 0 {
            continue;
        }
        // SAFETY: pour une entrée de type « accès autorisé », l'identifiant commence à l'octet 8.
        let sid = unsafe { entry.cast::<u8>().add(8) }.cast::<core::ffi::c_void>();
        if principals.classify(sid) == Owner::Other {
            return Ok(true);
        }
    }
    Ok(false)
}

/// Ouvre un répertoire sans suivre de point d'analyse, pour lire son propriétaire et sa liste.
pub fn open_directory(path: &Path) -> io::Result<File> {
    let name = wide(path);
    // SAFETY: le nom est terminé par un zéro ; aucun attribut de sécurité.
    let handle = unsafe {
        CreateFileW(
            name.as_ptr(),
            READ_CONTROL,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            null(),
            OPEN_EXISTING,
            FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT,
            null_mut(),
        )
    };
    if handle == INVALID_HANDLE_VALUE {
        return Err(last_error());
    }
    // SAFETY: CreateFileW a rendu une poignée que l'on possède.
    Ok(unsafe { File::from_raw_handle(handle) })
}

/// Un objet de tâche : tout processus qui y est rattaché, descendants compris, meurt avec lui.
pub struct Job {
    handle: OwnedHandle,
}

impl Job {
    /// Crée l'objet avec ses bornes : fermeture qui tue, mémoire par processus, temps processeur par
    /// processus (en secondes).
    pub fn new(memory_bytes: u64, cpu_seconds: u32) -> io::Result<Job> {
        // SAFETY: aucun attribut ni nom.
        let handle = unsafe { CreateJobObjectW(null(), null()) };
        if handle.is_null() {
            return Err(last_error());
        }
        // SAFETY: CreateJobObjectW a rendu une poignée que l'on possède.
        let handle = unsafe { OwnedHandle::from_raw_handle(handle) };
        // SAFETY: structure de pur C, pleine de zéros par défaut.
        let mut limits: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = unsafe { core::mem::zeroed() };
        limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE
            | JOB_OBJECT_LIMIT_PROCESS_MEMORY
            | JOB_OBJECT_LIMIT_PROCESS_TIME;
        limits.BasicLimitInformation.PerProcessUserTimeLimit =
            i64::from(cpu_seconds.max(1)).saturating_mul(10_000_000);
        limits.ProcessMemoryLimit = usize::try_from(memory_bytes).unwrap_or(usize::MAX);
        // SAFETY: la structure a la taille et le type annoncés.
        let done = unsafe {
            SetInformationJobObject(
                handle.as_raw_handle(),
                JobObjectExtendedLimitInformation,
                std::ptr::from_ref(&limits).cast(),
                u32::try_from(size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>()).unwrap_or(0),
            )
        };
        if done == 0 {
            return Err(last_error());
        }
        Ok(Job { handle })
    }

    /// Rattache un processus fils à l'objet.
    pub fn assign(&self, child: &Child) -> io::Result<()> {
        // SAFETY: les deux poignées sont valides.
        if unsafe { AssignProcessToJobObject(self.handle.as_raw_handle(), child.as_raw_handle()) }
            == 0
        {
            return Err(last_error());
        }
        Ok(())
    }

    /// Tue tous les processus de l'objet.
    pub fn terminate(&self) {
        // SAFETY: la poignée est valide.
        unsafe {
            TerminateJobObject(self.handle.as_raw_handle(), 1);
        }
    }
}
