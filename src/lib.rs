extern crate kernel32;
extern crate libc;
extern crate secur32;
extern crate winapi;

use kernel32::{FormatMessageW, LocalFree};
use libc::c_ulong;
use secur32::{AcquireCredentialsHandleA, FreeCredentialsHandle, InitializeSecurityContextW,
              DeleteSecurityContext, FreeContextBuffer, QueryContextAttributesW, DecryptMessage,
              EncryptMessage, ApplyControlToken};
use std::cmp;
use std::error;
use std::fmt;
use std::io::{self, BufRead, Read, Write, Cursor};
use std::mem;
use std::ops::Deref;
use std::ptr;
use std::result;
use std::slice;
use winapi::{CredHandle, DWORD, SECURITY_STATUS, SCHANNEL_CRED, SCHANNEL_CRED_VERSION,
             UNISP_NAME, SECPKG_CRED_OUTBOUND, SECPKG_CRED_INBOUND, SEC_E_OK, CtxtHandle,
             ISC_REQ_CONFIDENTIALITY, ISC_REQ_INTEGRITY, ISC_REQ_REPLAY_DETECT,
             ISC_REQ_SEQUENCE_DETECT, ISC_REQ_ALLOCATE_MEMORY, ISC_REQ_STREAM, SecBuffer,
             SECBUFFER_EMPTY, SECBUFFER_TOKEN, SecBufferDesc, SECBUFFER_VERSION,
             SEC_I_CONTINUE_NEEDED, SecPkgContext_StreamSizes, SECPKG_ATTR_STREAM_SIZES,
             SECBUFFER_ALERT, SECBUFFER_EXTRA, SEC_E_INCOMPLETE_MESSAGE, SECBUFFER_DATA,
             SECBUFFER_STREAM_HEADER, SECBUFFER_STREAM_TRAILER, SEC_I_CONTEXT_EXPIRED,
             SEC_I_RENEGOTIATE, SCHANNEL_SHUTDOWN, SEC_E_CONTEXT_EXPIRED,
             FORMAT_MESSAGE_ALLOCATE_BUFFER, FORMAT_MESSAGE_FROM_SYSTEM,
             FORMAT_MESSAGE_IGNORE_INSERTS, SCH_USE_STRONG_CRYPTO};

const INIT_REQUESTS: c_ulong = ISC_REQ_CONFIDENTIALITY |
                               ISC_REQ_INTEGRITY |
                               ISC_REQ_REPLAY_DETECT |
                               ISC_REQ_SEQUENCE_DETECT |
                               ISC_REQ_ALLOCATE_MEMORY |
                               ISC_REQ_STREAM;

pub type Result<T> = result::Result<T, Error>;

pub struct Error(SECURITY_STATUS);

impl fmt::Debug for Error {
    fn fmt(&self, fmt: &mut fmt::Formatter) -> fmt::Result {
        let mut builder = fmt.debug_struct("Error");
        builder.field("code", &format_args!("{:#x}", self.0));
        if let Some(message) = self.message() {
            builder.field("message", &message.trim());
        }
        builder.finish()
    }
}

impl fmt::Display for Error {
    fn fmt(&self, fmt: &mut fmt::Formatter) -> fmt::Result {
        match self.message() {
            Some(message) => fmt.write_str(message.trim()),
            None => write!(fmt, "unknown error {:#x}", self.0),
        }
    }
}

impl error::Error for Error {
    fn description(&self) -> &str {
        "an SChannel error"
    }
}

impl Error {
    fn into_io(self) -> io::Error {
        io::Error::new(io::ErrorKind::Other, self)
    }

    fn message(&self) -> Option<String> {
        unsafe {
            let mut buf: *mut u16 = ptr::null_mut();

            let flags = FORMAT_MESSAGE_ALLOCATE_BUFFER |
                FORMAT_MESSAGE_FROM_SYSTEM |
                FORMAT_MESSAGE_IGNORE_INSERTS;
            let ret = FormatMessageW(flags,
                                     ptr::null_mut(),
                                     self.0 as DWORD,
                                     0,
                                     &mut buf as *mut _ as *mut _,
                                     0,
                                     ptr::null_mut());

            if ret == 0 {
                return None;
            }

            let slice = slice::from_raw_parts(buf, ret as usize);
            let s = String::from_utf16(slice);

            LocalFree(buf as *mut _);

            s.ok()
        }
    }
}

pub enum Direction {
    Inbound,
    Outbound,
}

/// https://msdn.microsoft.com/en-us/library/windows/desktop/aa375549(v=vs.85).aspx
#[repr(u32)]
pub enum Algorithm {
    /// Advanced Encryption Standard (AES).
    Aes = winapi::CALG_AES,
    /// 128 bit AES.
    Aes128 = winapi::CALG_AES_128,
    /// 192 bit AES.
    Aes192 = winapi::CALG_AES_192,
    /// 256 bit AES.
    Aes256 = winapi::CALG_AES_256,
    /// Temporary algorithm identifier for handles of Diffie-Hellman–agreed keys.
    AgreedkeyAny = winapi::CALG_AGREEDKEY_ANY,
    /// An algorithm to create a 40-bit DES key that has parity bits and zeroed key bits to make
    /// its key length 64 bits.
    CylinkMek = winapi::CALG_CYLINK_MEK,
    /// DES encryption algorithm.
    Des = winapi::CALG_DES,
    /// DESX encryption algorithm.
    Desx = winapi::CALG_DESX,
    /// Diffie-Hellman ephemeral key exchange algorithm.
    DhEphem = winapi::CALG_DH_EPHEM,
    /// Diffie-Hellman store and forward key exchange algorithm.
    DhSf = winapi::CALG_DH_SF,
    /// DSA public key signature algorithm.
    DssSign = winapi::CALG_DSS_SIGN,
    /// Elliptic curve Diffie-Hellman key exchange algorithm.
    Ecdh = winapi::CALG_ECDH,
    // https://github.com/retep998/winapi-rs/issues/287
    // /// Ephemeral elliptic curve Diffie-Hellman key exchange algorithm.
    // EcdhEphem = winapi::CALG_ECDH_EPHEM,
    /// Elliptic curve digital signature algorithm.
    Ecdsa = winapi::CALG_ECDSA,
    /// One way function hashing algorithm.
    HashReplaceOwf = winapi::CALG_HASH_REPLACE_OWF,
    /// Hughes MD5 hashing algorithm.
    HughesMd5 = winapi::CALG_HUGHES_MD5,
    /// HMAC keyed hash algorithm.
    Hmac = winapi::CALG_HMAC,
    /// MAC keyed hash algorithm.
    Mac = winapi::CALG_MAC,
    /// MD2 hashing algorithm.
    Md2 = winapi::CALG_MD2,
    /// MD4 hashing algorithm.
    Md4 = winapi::CALG_MD4,
    /// MD5 hashing algorithm.
    Md5 = winapi::CALG_MD5,
    /// No signature algorithm..
    NoSign = winapi::CALG_NO_SIGN,
    /// RC2 block encryption algorithm.
    Rc2 = winapi::CALG_RC2,
    /// RC4 stream encryption algorithm.
    Rc4 = winapi::CALG_RC4,
    /// RC5 block encryption algorithm.
    Rc5 = winapi::CALG_RC5,
    /// RSA public key exchange algorithm.
    RsaKeyx = winapi::CALG_RSA_KEYX,
    /// RSA public key signature algorithm.
    RsaSign = winapi::CALG_RSA_SIGN,
    /// SHA hashing algorithm.
    Sha1 = winapi::CALG_SHA1,
    /// 256 bit SHA hashing algorithm.
    Sha256 = winapi::CALG_SHA_256,
    /// 384 bit SHA hashing algorithm.
    Sha384 = winapi::CALG_SHA_384,
    /// 512 bit SHA hashing algorithm.
    Sha512 = winapi::CALG_SHA_512,
    /// Triple DES encryption algorithm.
    TripleDes = winapi::CALG_3DES,
    /// Two-key triple DES encryption with effective key length equal to 112 bits.
    TripleDes112 = winapi::CALG_3DES_112,
}

pub struct SchannelCredBuilder {
    supported_algorithms: Option<Vec<Algorithm>>,
}

impl SchannelCredBuilder {
    pub fn new() -> SchannelCredBuilder {
        SchannelCredBuilder {
            supported_algorithms: None,
        }
    }

    /// Specify the supported algorithms for connections made with credentials produced by this
    /// builder. If no algorithms are specified (i.e. if this method isn't called or the `Vec` is
    /// empty) then Schannel uses the system defaults.
    pub fn with_supported_algorithms(mut self, supported_algorithms: Vec<Algorithm>)
                                     -> SchannelCredBuilder {
        self.supported_algorithms = Some(supported_algorithms);
        self
     }

    pub fn acquire(&self, direction: Direction) -> Result<SchannelCred> {
        unsafe {
            let mut handle = mem::uninitialized();
            let mut cred_data: SCHANNEL_CRED = mem::zeroed();
            cred_data.dwVersion = SCHANNEL_CRED_VERSION;
            cred_data.dwFlags = SCH_USE_STRONG_CRYPTO;
            if let Some(ref supported_algorithms) = self.supported_algorithms {
                cred_data.cSupportedAlgs = supported_algorithms.len() as DWORD;
                cred_data.palgSupportedAlgs = supported_algorithms.as_ptr() as *mut _;
            }

            let direction = match direction {
                Direction::Inbound => SECPKG_CRED_INBOUND,
                Direction::Outbound => SECPKG_CRED_OUTBOUND,
            };

            let mut unisp_name = UNISP_NAME.bytes().chain(Some(0u8)).collect::<Vec<u8>>();
            match AcquireCredentialsHandleA(ptr::null_mut(),
                                            unisp_name.as_mut_slice() as *mut _ as *mut _,
                                            direction,
                                            ptr::null_mut(),
                                            &mut cred_data as *mut _ as *mut _,
                                            None,
                                            ptr::null_mut(),
                                            &mut handle,
                                            ptr::null_mut()) {
                SEC_E_OK => Ok(SchannelCred(handle)),
                err => Err(Error(err)),
            }
        }
    }
}

pub struct SchannelCred(CredHandle);

impl Drop for SchannelCred {
    fn drop(&mut self) {
        unsafe {
            FreeCredentialsHandle(&mut self.0);
        }
    }
}

#[derive(Default)]
pub struct TlsStreamBuilder {
    domain: Option<Vec<u16>>,
}

impl TlsStreamBuilder {
    pub fn new() -> TlsStreamBuilder {
        TlsStreamBuilder::default()
    }

    pub fn domain(&mut self, domain: &str) -> &mut TlsStreamBuilder {
        self.domain = Some(domain.encode_utf16().chain(Some(0)).collect());
        self
    }

    pub fn initialize<S>(&self, cred: SchannelCred, stream: S) -> io::Result<TlsStream<S>>
        where S: Read + Write
    {
        let (ctxt, buf) = try!(SecurityContext::initialize(&cred,
                                                           self.domain.as_ref().map(|s| &s[..]))
                                   .map_err(Error::into_io));

        let mut stream = TlsStream {
            cred: cred,
            context: ctxt,
            domain: self.domain.clone(),
            stream: stream,
            state: State::Initializing {
                needs_flush: false,
                more_calls: true,
                shutting_down: false,
            },
            needs_read: true,
            dec_in: Cursor::new(Vec::new()),
            enc_in: Cursor::new(Vec::new()),
            out_buf: Cursor::new(buf.to_owned()),
        };
        try!(stream.initialize());

        Ok(stream)
    }
}

struct SecurityContext(CtxtHandle);

impl SecurityContext {
    fn initialize(cred: &SchannelCred,
                  domain: Option<&[u16]>)
                  -> Result<(SecurityContext, ContextBuffer)> {
        unsafe {
            let domain = domain.map(|b| b.as_ptr() as *mut u16).unwrap_or(ptr::null_mut());

            let mut ctxt = mem::uninitialized();

            let mut outbuf = SecBuffer {
                cbBuffer: 0,
                BufferType: SECBUFFER_EMPTY,
                pvBuffer: ptr::null_mut(),
            };
            let mut outbuf_desc = SecBufferDesc {
                ulVersion: SECBUFFER_VERSION,
                cBuffers: 1,
                pBuffers: &mut outbuf,
            };

            let mut attributes = 0;

            match InitializeSecurityContextW(&cred.0 as *const _ as *mut _,
                                             ptr::null_mut(),
                                             domain,
                                             INIT_REQUESTS,
                                             0,
                                             0,
                                             ptr::null_mut(),
                                             0,
                                             &mut ctxt,
                                             &mut outbuf_desc,
                                             &mut attributes,
                                             ptr::null_mut()) {
                SEC_I_CONTINUE_NEEDED => Ok((SecurityContext(ctxt), ContextBuffer(outbuf))),
                err => Err(Error(err)),
            }
        }
    }

    fn stream_sizes(&mut self) -> Result<SecPkgContext_StreamSizes> {
        unsafe {
            let mut stream_sizes = mem::uninitialized();
            let status = QueryContextAttributesW(&mut self.0,
                                                 SECPKG_ATTR_STREAM_SIZES,
                                                 &mut stream_sizes as *mut _ as *mut _);
            if status == SEC_E_OK {
                Ok(stream_sizes)
            } else {
                Err(Error(status))
            }
        }
    }
}

impl Drop for SecurityContext {
    fn drop(&mut self) {
        unsafe {
            DeleteSecurityContext(&mut self.0);
        }
    }
}

struct ContextBuffer(SecBuffer);

impl Drop for ContextBuffer {
    fn drop(&mut self) {
        unsafe {
            FreeContextBuffer(self.0.pvBuffer);
        }
    }
}

impl Deref for ContextBuffer {
    type Target = [u8];

    fn deref(&self) -> &[u8] {
        unsafe { slice::from_raw_parts(self.0.pvBuffer as *const _, self.0.cbBuffer as usize) }
    }
}

enum State {
    Initializing {
        needs_flush: bool,
        more_calls: bool,
        shutting_down: bool,
    },
    Streaming {
        sizes: SecPkgContext_StreamSizes,
    },
    Shutdown,
}

pub struct TlsStream<S> {
    cred: SchannelCred,
    context: SecurityContext,
    domain: Option<Vec<u16>>,
    stream: S,
    state: State,
    needs_read: bool,
    // valid from position() to len()
    dec_in: Cursor<Vec<u8>>,
    // valid from 0 to position()
    enc_in: Cursor<Vec<u8>>,
    // valid from position() to len()
    out_buf: Cursor<Vec<u8>>,
}

impl<S> TlsStream<S>
    where S: Read + Write
{
    pub fn get_ref(&self) -> &S {
        &self.stream
    }

    pub fn get_mut(&mut self) -> &mut S {
        &mut self.stream
    }

    pub fn shutdown(&mut self) -> io::Result<()> {
        match self.state {
            State::Shutdown => return Ok(()),
            State::Initializing { shutting_down: true, .. } => {},
            _ => {
                unsafe {
                    let mut token = SCHANNEL_SHUTDOWN;
                    let mut buf = SecBuffer {
                        cbBuffer: mem::size_of_val(&token) as c_ulong,
                        BufferType: SECBUFFER_TOKEN,
                        pvBuffer: &mut token as *mut _ as *mut _,
                    };
                    let mut desc = SecBufferDesc {
                        ulVersion: SECBUFFER_VERSION,
                        cBuffers: 1,
                        pBuffers: &mut buf,
                    };
                    match ApplyControlToken(&mut self.context.0, &mut desc) {
                        SEC_E_OK => {},
                        err => return Err(Error(err).into_io()),
                    }
                }

                self.state = State::Initializing {
                    needs_flush: false,
                    more_calls: true,
                    shutting_down: true,
                };
                self.needs_read = false;
            }
        }

        self.initialize().map(|_| ())
    }

    fn step_initialize(&mut self) -> Result<()> {
        unsafe {
            let domain = self.domain
                             .as_ref()
                             .map(|b| b.as_ptr() as *mut u16)
                             .unwrap_or(ptr::null_mut());

            let inbufs = &mut [SecBuffer {
                                   cbBuffer: self.enc_in.position() as c_ulong,
                                   BufferType: SECBUFFER_TOKEN,
                                   pvBuffer: self.enc_in.get_mut().as_mut_ptr() as *mut _,
                               },
                               SecBuffer {
                                   cbBuffer: 0,
                                   BufferType: SECBUFFER_EMPTY,
                                   pvBuffer: ptr::null_mut(),
                               }];
            let mut inbuf_desc = SecBufferDesc {
                ulVersion: SECBUFFER_VERSION,
                cBuffers: 2,
                pBuffers: inbufs.as_mut_ptr(),
            };

            let outbufs = &mut [SecBuffer {
                                    cbBuffer: 0,
                                    BufferType: SECBUFFER_TOKEN,
                                    pvBuffer: ptr::null_mut(),
                                },
                                SecBuffer {
                                    cbBuffer: 0,
                                    BufferType: SECBUFFER_ALERT,
                                    pvBuffer: ptr::null_mut(),
                                }];
            let mut outbuf_desc = SecBufferDesc {
                ulVersion: SECBUFFER_VERSION,
                cBuffers: 2,
                pBuffers: outbufs.as_mut_ptr(),
            };

            let mut attributes = 0;

            let status = InitializeSecurityContextW(&mut self.cred.0,
                                                    &mut self.context.0,
                                                    domain,
                                                    INIT_REQUESTS,
                                                    0,
                                                    0,
                                                    &mut inbuf_desc,
                                                    0,
                                                    ptr::null_mut(),
                                                    &mut outbuf_desc,
                                                    &mut attributes,
                                                    ptr::null_mut());

            if !outbufs[1].pvBuffer.is_null() {
                FreeContextBuffer(outbufs[1].pvBuffer);
            }

            match status {
                SEC_I_CONTINUE_NEEDED => {
                    let nread = if inbufs[1].BufferType == SECBUFFER_EXTRA {
                        self.enc_in.position() as usize - inbufs[1].cbBuffer as usize
                    } else {
                        self.enc_in.position() as usize
                    };
                    let to_write = ContextBuffer(outbufs[0]);

                    self.consume_enc_in(nread);
                    self.needs_read = self.enc_in.position() == 0;
                    self.out_buf.get_mut().extend_from_slice(&to_write);
                }
                SEC_E_INCOMPLETE_MESSAGE => self.needs_read = true,
                SEC_E_OK => {
                    let nread = if inbufs[1].BufferType == SECBUFFER_EXTRA {
                        self.enc_in.position() as usize - inbufs[1].cbBuffer as usize
                    } else {
                        self.enc_in.position() as usize
                    };
                    let to_write = if outbufs[0].pvBuffer.is_null() {
                        None
                    } else {
                        Some(ContextBuffer(outbufs[0]))
                    };

                    self.consume_enc_in(nread);
                    self.needs_read = self.enc_in.position() == 0;
                    if let Some(to_write) = to_write {
                        self.out_buf.get_mut().extend_from_slice(&to_write);
                    }
                    if self.enc_in.position() != 0 {
                        try!(self.decrypt());
                    }
                    if let State::Initializing { ref mut more_calls, .. } = self.state {
                        *more_calls = false;
                    }
                }
                _ => return Err(Error(status)),
            }
            Ok(())
        }
    }

    fn initialize(&mut self) -> io::Result<Option<SecPkgContext_StreamSizes>> {
        loop {
            match self.state {
                State::Initializing { mut needs_flush, more_calls, shutting_down } => {
                    if try!(self.write_out()) > 0 {
                        needs_flush = true;
                        if let State::Initializing { needs_flush: ref mut n, .. } = self.state {
                            *n = needs_flush;
                        }
                    }

                    if needs_flush {
                        try!(self.stream.flush());
                        if let State::Initializing { ref mut needs_flush, .. } = self.state {
                            *needs_flush = false;
                        }
                    }

                    if !more_calls {
                        self.state = if shutting_down {
                            State::Shutdown
                        } else {
                            State::Streaming {
                                sizes: try!(self.context.stream_sizes().map_err(Error::into_io)),
                            }
                        };

                        continue;
                    }

                    if self.needs_read {
                        if try!(self.read_in()) == 0 {
                            return Err(io::Error::new(io::ErrorKind::UnexpectedEof,
                                                      "unexpected EOF during handshake"));
                        }
                    }

                    try!(self.step_initialize().map_err(Error::into_io));
                }
                State::Streaming { sizes } => return Ok(Some(sizes)),
                State::Shutdown => return Ok(None),
            }
        }
    }

    fn write_out(&mut self) -> io::Result<usize> {
        let mut out = 0;
        while self.out_buf.position() as usize != self.out_buf.get_ref().len() {
            let position = self.out_buf.position() as usize;
            let nwritten = try!(self.stream.write(&self.out_buf.get_ref()[position..]));
            out += nwritten;
            self.out_buf.set_position((position + nwritten) as u64);
        }

        Ok(out)
    }

    fn read_in(&mut self) -> io::Result<usize> {
        let existing_len = self.enc_in.position() as usize;
        let min_len = cmp::max(1024, 2 * existing_len);
        if self.enc_in.get_ref().len() < min_len {
            self.enc_in.get_mut().resize(min_len, 0);
        }
        let nread = {
            let buf = &mut self.enc_in.get_mut()[existing_len..];
            try!(self.stream.read(buf))
        };
        self.enc_in.set_position((existing_len + nread) as u64);
        Ok(nread)
    }

    fn consume_enc_in(&mut self, nread: usize) {
        unsafe {
            let src = &self.enc_in.get_ref()[nread] as *const _;
            let dst = self.enc_in.get_mut().as_mut_ptr();

            let size = self.enc_in.position() as usize;
            assert!(size >= nread);
            let count = size - nread;

            ptr::copy(src, dst, count);

            self.enc_in.set_position(count as u64);
        }
    }

    fn decrypt(&mut self) -> Result<()> {
        unsafe {
            let bufs = &mut [SecBuffer {
                                 cbBuffer: self.enc_in.position() as c_ulong,
                                 BufferType: SECBUFFER_DATA,
                                 pvBuffer: self.enc_in.get_mut().as_mut_ptr() as *mut _,
                             },
                             SecBuffer {
                                 cbBuffer: 0,
                                 BufferType: SECBUFFER_EMPTY,
                                 pvBuffer: ptr::null_mut(),
                             },
                             SecBuffer {
                                 cbBuffer: 0,
                                 BufferType: SECBUFFER_EMPTY,
                                 pvBuffer: ptr::null_mut(),
                             },
                             SecBuffer {
                                 cbBuffer: 0,
                                 BufferType: SECBUFFER_EMPTY,
                                 pvBuffer: ptr::null_mut(),
                             }];
            let mut bufdesc = SecBufferDesc {
                ulVersion: SECBUFFER_VERSION,
                cBuffers: 4,
                pBuffers: bufs.as_mut_ptr(),
            };

            match DecryptMessage(&mut self.context.0, &mut bufdesc, 0, ptr::null_mut()) {
                SEC_E_OK => {
                    let start = bufs[1].pvBuffer as usize - self.enc_in.get_ref().as_ptr() as usize;
                    let end = start + bufs[1].cbBuffer as usize;
                    self.dec_in.get_mut().clear();
                    self.dec_in
                        .get_mut()
                        .extend_from_slice(&self.enc_in.get_ref()[start..end]);
                    self.dec_in.set_position(0);

                    let nread = if bufs[3].BufferType == SECBUFFER_EXTRA {
                        self.enc_in.position() as usize - bufs[3].cbBuffer as usize
                    } else {
                        self.enc_in.position() as usize
                    };
                    self.consume_enc_in(nread);
                    self.needs_read = self.enc_in.position() == 0;
                    Ok(())
                }
                SEC_E_INCOMPLETE_MESSAGE => {
                    self.needs_read = true;
                    Ok(())
                }
                state @ SEC_I_CONTEXT_EXPIRED |
                state @ SEC_I_RENEGOTIATE => {
                    self.state = State::Initializing {
                        needs_flush: false,
                        more_calls: true,
                        shutting_down: state == SEC_I_CONTEXT_EXPIRED,
                    };

                    let nread = if bufs[3].BufferType == SECBUFFER_EXTRA {
                        self.enc_in.position() as usize - bufs[3].cbBuffer as usize
                    } else {
                        self.enc_in.position() as usize
                    };
                    self.consume_enc_in(nread);
                    self.needs_read = self.enc_in.position() == 0;
                    Ok(())
                }
                e => Err(Error(e)),
            }
        }
    }

    fn encrypt(&mut self, buf: &[u8], sizes: &SecPkgContext_StreamSizes) -> Result<()> {
        assert!(buf.len() <= sizes.cbMaximumMessage as usize);

        unsafe {
            let len = sizes.cbHeader as usize + buf.len() + sizes.cbTrailer as usize;

            if self.out_buf.get_ref().len() < len {
                self.out_buf.get_mut().resize(len, 0);
            }

            let message_start = sizes.cbHeader as usize;
            self.out_buf
                .get_mut()[message_start..message_start + buf.len()]
                .clone_from_slice(buf);

            let buf_start = self.out_buf.get_mut().as_mut_ptr();
            let bufs = &mut [SecBuffer {
                                 cbBuffer: sizes.cbHeader,
                                 BufferType: SECBUFFER_STREAM_HEADER,
                                 pvBuffer: buf_start as *mut _,
                             },
                             SecBuffer {
                                 cbBuffer: buf.len() as c_ulong,
                                 BufferType: SECBUFFER_DATA,
                                 pvBuffer: buf_start.offset(sizes.cbHeader as isize) as *mut _,
                             },
                             SecBuffer {
                                 cbBuffer: sizes.cbTrailer,
                                 BufferType: SECBUFFER_STREAM_TRAILER,
                                 pvBuffer: buf_start.offset(sizes.cbHeader as isize + buf.len() as isize) as *mut _,
                             },
                             SecBuffer {
                                 cbBuffer: 0,
                                 BufferType: SECBUFFER_EMPTY,
                                 pvBuffer: ptr::null_mut(),
                             }];
            let mut bufdesc = SecBufferDesc {
                ulVersion: SECBUFFER_VERSION,
                cBuffers: 4,
                pBuffers: bufs.as_mut_ptr(),
            };

            match EncryptMessage(&mut self.context.0, 0, &mut bufdesc, 0) {
                SEC_E_OK => {
                    let len = bufs[0].cbBuffer + bufs[1].cbBuffer + bufs[2].cbBuffer;
                    self.out_buf.get_mut().truncate(len as usize);
                    self.out_buf.set_position(0);
                    Ok(())
                }
                err => Err(Error(err)),
            }
        }
    }

    fn get_buf(&self) -> &[u8] {
        &self.dec_in.get_ref()[self.dec_in.position() as usize..]
    }
}

impl<S> Write for TlsStream<S>
    where S: Read + Write
{
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let sizes = match try!(self.initialize()) {
            Some(sizes) => sizes,
            None => return Err(Error(SEC_E_CONTEXT_EXPIRED).into_io()),
        };

        let len = cmp::min(buf.len(), sizes.cbMaximumMessage as usize);

        // if we have pending output data, it must have been because a previous
        // attempt to send this data ran into an error. Specifically in the
        // case of WouldBlock errors, we expect another call to write with the
        // same data.
        if self.out_buf.position() == self.out_buf.get_ref().len() as u64 {
            try!(self.encrypt(&buf[..len], &sizes).map_err(Error::into_io));
        }
        try!(self.write_out());

        Ok(len)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.stream.flush()
    }
}

impl<S> Read for TlsStream<S>
    where S: Read + Write
{
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let nread = {
            let read_buf = try!(self.fill_buf());
            let nread = cmp::min(buf.len(), read_buf.len());
            buf[..nread].clone_from_slice(&read_buf[..nread]);
            nread
        };
        self.consume(nread);
        Ok(nread)
    }
}

impl<S> BufRead for TlsStream<S>
    where S: Read + Write
{
    fn fill_buf(&mut self) -> io::Result<&[u8]> {
        while self.get_buf().is_empty() {
            if let State::Shutdown = self.state {
                break;
            }

            if self.needs_read {
                if try!(self.read_in()) == 0 {
                    break;
                }
                self.needs_read = false;
            }

            try!(self.decrypt().map_err(Error::into_io));
        }

        Ok(self.get_buf())
    }

    fn consume(&mut self, amt: usize) {
        let pos = self.dec_in.position() + amt as u64;
        assert!(pos <= self.dec_in.get_ref().len() as u64);
        self.dec_in.set_position(pos);
    }
}

#[cfg(test)]
mod test {
    use std::io::{Read, Write};
    use std::net::TcpStream;

    use super::*;
    use winapi;

    #[test]
    fn basic() {
        let creds = SchannelCredBuilder::new().acquire(Direction::Outbound).unwrap();
        let stream = TcpStream::connect("google.com:443").unwrap();
        let mut stream = TlsStreamBuilder::new()
                             .domain("google.com")
                             .initialize(creds, stream)
                             .unwrap();
        stream.write_all(b"GET / HTTP/1.0\r\n\r\n").unwrap();
        let mut out = vec![];
        stream.read_to_end(&mut out).unwrap();
        assert!(out.starts_with(b"HTTP/1.0 200 OK"));
        assert!(out.ends_with(b"</html>"));
    }

    #[test]
    #[allow(overflowing_literals)]
    fn invalid_algorithms() {
        let algorithms = vec![
            Algorithm::Rc2,
            Algorithm::Ecdsa,
        ];
        let creds = SchannelCredBuilder::new()
                        .with_supported_algorithms(algorithms)
                        .acquire(Direction::Outbound);
        assert_eq!(creds.err().unwrap().0, winapi::SEC_E_ALGORITHM_MISMATCH);
    }

    #[test]
    fn valid_algorithms() {
        let algorithms = vec![
            Algorithm::Aes128,
            Algorithm::Ecdsa,
        ];
        let creds = SchannelCredBuilder::new()
                        .with_supported_algorithms(algorithms)
                        .acquire(Direction::Outbound)
                        .unwrap();
        let stream = TcpStream::connect("google.com:443").unwrap();
        let mut stream = TlsStreamBuilder::new()
                             .domain("google.com")
                             .initialize(creds, stream)
                             .unwrap();
        stream.write_all(b"GET / HTTP/1.0\r\n\r\n").unwrap();
        let mut out = vec![];
        stream.read_to_end(&mut out).unwrap();
        assert!(out.starts_with(b"HTTP/1.0 200 OK"));
        assert!(out.ends_with(b"</html>"));
    }

    #[test]
    fn bad_domain() {
        let creds = SchannelCredBuilder::new().acquire(Direction::Outbound).unwrap();
        let stream = TcpStream::connect("google.com:443").unwrap();
        TlsStreamBuilder::new()
            .domain("foobar.com")
            .initialize(creds, stream)
            .err()
            .unwrap();
    }

    #[test]
    fn shutdown() {
        let creds = SchannelCredBuilder::new().acquire(Direction::Outbound).unwrap();
        let stream = TcpStream::connect("google.com:443").unwrap();
        let mut stream = TlsStreamBuilder::new()
                             .domain("google.com")
                             .initialize(creds, stream)
                             .unwrap();
        stream.shutdown().unwrap();
    }
}
