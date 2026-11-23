//! What an automation client may do (ADR 0072).
//!
//! Every method belongs to one [`Grant`]. A session starts with the
//! read-only [`Grants::DEFAULT`] set unless the user who launched it names
//! the grants with `--devtools-allow`. The grants that let a client change
//! the page or learn more than it shows (running script, pausing it,
//! synthesizing trusted input, reading network diagnostics) are never on
//! by default.

/// A capability a session may be given.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub enum Grant {
    /// Read the DOM, computed style, layout and screenshots.
    Inspect,
    /// Read console messages.
    Console,
    /// Read performance counters and the statement profile.
    Profile,
    /// Read the page's subresource requests (redacted).
    Network,
    /// Deliver trusted input, which gives the page user activation.
    Input,
    /// Run script in the page.
    Evaluate,
    /// Set breakpoints, pause and step page script.
    Debug,
}

impl Grant {
    /// Every grant, in protocol order.
    pub const ALL: [Grant; 7] = [
        Grant::Inspect,
        Grant::Console,
        Grant::Profile,
        Grant::Network,
        Grant::Input,
        Grant::Evaluate,
        Grant::Debug,
    ];

    /// The name used on the command line and in the protocol.
    pub fn name(self) -> &'static str {
        match self {
            Grant::Inspect => "inspect",
            Grant::Console => "console",
            Grant::Profile => "profile",
            Grant::Network => "network",
            Grant::Input => "input",
            Grant::Evaluate => "evaluate",
            Grant::Debug => "debug",
        }
    }

    /// The grant with this name.
    pub fn from_name(name: &str) -> Option<Grant> {
        Grant::ALL.into_iter().find(|grant| grant.name() == name)
    }

    fn bit(self) -> u8 {
        1 << Grant::ALL.iter().position(|&g| g == self).unwrap_or(0)
    }
}

/// A set of grants.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub struct Grants(u8);

impl Grants {
    /// No grants: only the always-available session and page lifecycle
    /// methods.
    pub const NONE: Grants = Grants(0);

    /// The read-only default: inspect, console and profile.
    pub const DEFAULT: Grants = Grants(0b0000_0111);

    /// Parse a comma-separated list such as `inspect,evaluate`; `none`
    /// alone is the empty set. Unknown or repeated names are errors.
    pub fn parse(list: &str) -> Result<Grants, String> {
        if list == "none" {
            return Ok(Grants::NONE);
        }
        let mut grants = Grants::NONE;
        for name in list.split(',') {
            let grant = Grant::from_name(name.trim())
                .ok_or_else(|| format!("unknown DevTools grant {name:?}"))?;
            if grants.contains(grant) {
                return Err(format!("DevTools grant {name:?} is named twice"));
            }
            grants = grants.with(grant);
        }
        Ok(grants)
    }

    /// This set plus `grant`.
    pub fn with(self, grant: Grant) -> Grants {
        Grants(self.0 | grant.bit())
    }

    /// Whether the set holds `grant`.
    pub fn contains(self, grant: Grant) -> bool {
        self.0 & grant.bit() != 0
    }

    /// The grants held, in protocol order.
    pub fn iter(self) -> impl Iterator<Item = Grant> {
        Grant::ALL.into_iter().filter(move |&g| self.contains(g))
    }

    /// The bit set, for passing to a renderer.
    pub fn bits(self) -> u8 {
        self.0
    }

    /// The set from [`Self::bits`]; unknown bits are dropped.
    pub fn from_bits(bits: u8) -> Grants {
        Grants(bits & 0b0111_1111)
    }
}

/// Every protocol method and the grant it needs (`None`: always allowed).
pub const METHODS: &[(&str, Option<Grant>)] = &[
    ("session.capabilities", None),
    ("session.close", None),
    ("page.load", None),
    ("page.info", None),
    ("page.settle", None),
    ("page.advance", None),
    ("page.screenshot", Some(Grant::Inspect)),
    ("dom.document", Some(Grant::Inspect)),
    ("dom.query", Some(Grant::Inspect)),
    ("dom.attributes", Some(Grant::Inspect)),
    ("dom.text", Some(Grant::Inspect)),
    ("css.computed", Some(Grant::Inspect)),
    ("layout.box", Some(Grant::Inspect)),
    ("layout.hitTest", Some(Grant::Inspect)),
    ("console.messages", Some(Grant::Console)),
    ("profile.metrics", Some(Grant::Profile)),
    ("profile.statements", Some(Grant::Profile)),
    ("network.requests", Some(Grant::Network)),
    ("input.click", Some(Grant::Input)),
    ("runtime.evaluate", Some(Grant::Evaluate)),
    ("debugger.scripts", Some(Grant::Debug)),
    ("debugger.setBreakpoint", Some(Grant::Debug)),
    ("debugger.removeBreakpoint", Some(Grant::Debug)),
    ("debugger.pause", Some(Grant::Debug)),
    ("debugger.resume", Some(Grant::Debug)),
    ("debugger.stepInto", Some(Grant::Debug)),
    ("debugger.stepOver", Some(Grant::Debug)),
    ("debugger.stepOut", Some(Grant::Debug)),
    ("debugger.stack", Some(Grant::Debug)),
    ("debugger.scopes", Some(Grant::Debug)),
];

/// The grant `method` needs: `Some(None)` for an always-allowed method,
/// `None` for an unknown one.
pub fn required_grant(method: &str) -> Option<Option<Grant>> {
    METHODS
        .iter()
        .find(|(name, _)| *name == method)
        .map(|&(_, grant)| grant)
}
