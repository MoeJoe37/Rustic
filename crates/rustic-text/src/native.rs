//! Where each desktop OS keeps the fonts its own browsers use for the CSS
//! generic families (platform.native, TD-85). This is data only: it opens
//! nothing, and a host decides which of these files to read and grant.
//! On Linux and the BSDs the host asks fontconfig first (the platform's
//! font service) and uses these paths only when it is absent.

/// Candidate files for one generic family: sets tried in order, each set
/// regular first, then bold, italic and bold italic where the family has
/// them. A set is used when its first file exists.
#[derive(Clone, Copy, Debug)]
pub struct Candidate {
    /// The CSS generic family (`sans-serif`, `serif`, ...).
    pub generic: &'static str,
    /// Family names to ask the platform font service for (DirectWrite,
    /// Core Text; `rustic_native`), in order, before the file sets.
    pub families: &'static [&'static str],
    /// File sets, relative to one of [`Native::roots`].
    pub sets: &'static [&'static [&'static str]],
}

/// One platform's font locations.
#[derive(Clone, Copy, Debug)]
pub struct Native {
    /// Font directories, searched in order. `%WINDIR%` stands for the
    /// Windows directory; `~` for the user's home.
    pub roots: &'static [&'static str],
    /// Generic family candidates; `sans-serif` first (the fallback family).
    pub generics: &'static [Candidate],
    /// Extra faces for characters the generic families lack (symbols,
    /// CJK, other scripts), in fallback order.
    pub fallbacks: &'static [&'static str],
}

const WINDOWS: Native = Native {
    roots: &[
        "%WINDIR%\\Fonts",
        "~\\AppData\\Local\\Microsoft\\Windows\\Fonts",
    ],
    generics: &[
        Candidate {
            generic: "sans-serif",
            families: &["Arial"],
            sets: &[&["arial.ttf", "arialbd.ttf", "ariali.ttf", "arialbi.ttf"]],
        },
        Candidate {
            generic: "serif",
            families: &["Times New Roman"],
            sets: &[&["times.ttf", "timesbd.ttf", "timesi.ttf", "timesbi.ttf"]],
        },
        Candidate {
            generic: "monospace",
            families: &["Consolas", "Courier New"],
            sets: &[
                &[
                    "consola.ttf",
                    "consolab.ttf",
                    "consolai.ttf",
                    "consolaz.ttf",
                ],
                &["cour.ttf", "courbd.ttf", "couri.ttf", "courbi.ttf"],
            ],
        },
        Candidate {
            generic: "system-ui",
            families: &["Segoe UI"],
            sets: &[&[
                "segoeui.ttf",
                "segoeuib.ttf",
                "segoeuii.ttf",
                "segoeuiz.ttf",
            ]],
        },
        Candidate {
            generic: "cursive",
            families: &["Comic Sans MS"],
            sets: &[&["comic.ttf", "comicbd.ttf"]],
        },
        Candidate {
            generic: "fantasy",
            families: &["Impact"],
            sets: &[&["impact.ttf"]],
        },
        Candidate {
            generic: "emoji",
            families: &["Segoe UI Emoji"],
            sets: &[&["seguiemj.ttf"]],
        },
    ],
    fallbacks: &[
        "seguisym.ttf",
        "malgun.ttf",
        "msgothic.ttc",
        "Nirmala.ttc",
        "Nirmala.ttf",
    ],
};

const MACOS: Native = Native {
    roots: &[
        "/System/Library/Fonts/Supplemental",
        "/System/Library/Fonts",
        "/Library/Fonts",
        "~/Library/Fonts",
    ],
    generics: &[
        Candidate {
            generic: "sans-serif",
            families: &["Arial", "Helvetica"],
            sets: &[
                &[
                    "Arial.ttf",
                    "Arial Bold.ttf",
                    "Arial Italic.ttf",
                    "Arial Bold Italic.ttf",
                ],
                &["Helvetica.ttc"],
            ],
        },
        Candidate {
            generic: "serif",
            families: &["Times New Roman", "Times"],
            sets: &[
                &[
                    "Times New Roman.ttf",
                    "Times New Roman Bold.ttf",
                    "Times New Roman Italic.ttf",
                    "Times New Roman Bold Italic.ttf",
                ],
                &["Times.ttc"],
            ],
        },
        Candidate {
            generic: "monospace",
            families: &["Courier New", "Menlo"],
            sets: &[
                &[
                    "Courier New.ttf",
                    "Courier New Bold.ttf",
                    "Courier New Italic.ttf",
                    "Courier New Bold Italic.ttf",
                ],
                &["Menlo.ttc"],
            ],
        },
        Candidate {
            generic: "system-ui",
            families: &[],
            sets: &[&["SFNS.ttf"], &["Helvetica.ttc"]],
        },
        Candidate {
            generic: "cursive",
            families: &["Comic Sans MS"],
            sets: &[&["Comic Sans MS.ttf", "Comic Sans MS Bold.ttf"]],
        },
        Candidate {
            generic: "fantasy",
            families: &["Impact"],
            sets: &[&["Impact.ttf"]],
        },
    ],
    fallbacks: &["Arial Unicode.ttf", "Apple Symbols.ttf"],
};

const UNIX: Native = Native {
    roots: &[
        "/usr/share/fonts/truetype",
        "/usr/share/fonts/TTF",
        "/usr/share/fonts",
        "/usr/local/share/fonts",
        "~/.local/share/fonts",
        "~/.fonts",
    ],
    generics: &[
        Candidate {
            generic: "sans-serif",
            families: &[],
            sets: &[
                &[
                    "dejavu/DejaVuSans.ttf",
                    "dejavu/DejaVuSans-Bold.ttf",
                    "dejavu/DejaVuSans-Oblique.ttf",
                    "dejavu/DejaVuSans-BoldOblique.ttf",
                ],
                &[
                    "liberation/LiberationSans-Regular.ttf",
                    "liberation/LiberationSans-Bold.ttf",
                    "liberation/LiberationSans-Italic.ttf",
                    "liberation/LiberationSans-BoldItalic.ttf",
                ],
                &["noto/NotoSans-Regular.ttf", "noto/NotoSans-Bold.ttf"],
            ],
        },
        Candidate {
            generic: "serif",
            families: &[],
            sets: &[
                &[
                    "dejavu/DejaVuSerif.ttf",
                    "dejavu/DejaVuSerif-Bold.ttf",
                    "dejavu/DejaVuSerif-Italic.ttf",
                    "dejavu/DejaVuSerif-BoldItalic.ttf",
                ],
                &[
                    "liberation/LiberationSerif-Regular.ttf",
                    "liberation/LiberationSerif-Bold.ttf",
                ],
            ],
        },
        Candidate {
            generic: "monospace",
            families: &[],
            sets: &[
                &[
                    "dejavu/DejaVuSansMono.ttf",
                    "dejavu/DejaVuSansMono-Bold.ttf",
                    "dejavu/DejaVuSansMono-Oblique.ttf",
                    "dejavu/DejaVuSansMono-BoldOblique.ttf",
                ],
                &[
                    "liberation/LiberationMono-Regular.ttf",
                    "liberation/LiberationMono-Bold.ttf",
                ],
            ],
        },
    ],
    fallbacks: &[],
};

/// This platform's font locations (Linux and the BSDs share one table;
/// Android and iOS hosts are M14 and use the Unix table until then).
pub fn native() -> Native {
    if cfg!(target_os = "windows") {
        WINDOWS
    } else if cfg!(target_os = "macos") {
        MACOS
    } else {
        UNIX
    }
}

/// The CSS generic families a host should map, in the order it maps them.
pub const GENERICS: [&str; 13] = [
    "sans-serif",
    "serif",
    "monospace",
    "cursive",
    "fantasy",
    "system-ui",
    "ui-serif",
    "ui-sans-serif",
    "ui-monospace",
    "ui-rounded",
    "emoji",
    "math",
    "fangsong",
];

/// The generic a `ui-*` or other unlisted generic borrows when the
/// platform has no family of its own for it.
pub fn borrowed(generic: &str) -> &'static str {
    match generic {
        "ui-serif" | "fangsong" | "math" => "serif",
        "ui-monospace" => "monospace",
        "system-ui" | "ui-sans-serif" | "ui-rounded" | "emoji" => "sans-serif",
        _ => "sans-serif",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_platform_maps_the_three_basic_generics_first() {
        for native in [WINDOWS, MACOS, UNIX] {
            let generics: Vec<&str> = native.generics.iter().map(|c| c.generic).collect();
            assert_eq!(&generics[..3], &["sans-serif", "serif", "monospace"]);
            for candidate in native.generics {
                assert!(GENERICS.contains(&candidate.generic));
                assert!(candidate.sets.iter().all(|set| !set.is_empty()));
            }
            assert!(!native.roots.is_empty());
        }
        assert_eq!(borrowed("ui-monospace"), "monospace");
        assert_eq!(borrowed("system-ui"), "sans-serif");
    }
}
