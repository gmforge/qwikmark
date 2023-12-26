use nom::branch::alt;
use nom::bytes::complete::{is_a, is_not, tag, take_while1, take_while_m_n};
use nom::character::complete::{
    alpha1, anychar, char, digit1, line_ending, not_line_ending, space0, space1,
};
use nom::character::is_digit;
use nom::combinator::{cond, consumed, eof, not, opt, peek};
use nom::multi::{many0, many1, separated_list1};
use nom::sequence::{delimited, preceded, separated_pair, terminated, tuple};
use nom::IResult;
use phf::phf_map;
use std::collections::HashMap;

// Keep track of block starts, especially blocks off of root as they represent contained sections
// of isolated changes. These start points are important for long logs where only want to render
// a section of the document and know that any previous text before the start point will not
// impact current text being rendered.

// Utilize nom nested checks for both Text content expansion and edge tag building.
// For edge checks have either last was whitespace or word.
// NOTE: Either Open brackets are under whitespace and closing brackets are under word. Or
//   They do not change last status. Need to look over cases to determin.
// If last was whitespace and on edge tag then
//   1) check for current closing tag and if encountered do not start edge tag content
//   2) If word or opening bracket tag encountered as next character, start edge tag content
//   3) If another edge tag then stack until find word or opening bracket.
//
// Block::Paragraph is the default Block that captures text in te form of Spans
// Span::Text is the default span/tag that joins char runs

// ATTRIBUTES

// Format     = { "=" ~ Field }
// Identifier = { "#" ~ Field }
// Class      = { "." ~ Field }
// Attribute  = { Format | "{" ~ " "* ~ ((Format | Identifier | Class) ~ " "*)+ ~ " "* ~ "}" }
fn key<'a>(input: &'a str) -> IResult<&'a str, &'a str> {
    // let (input, (k, _)) = consumed(tuple((alpha1, many0(alt((alphanumeric0, is_a("_")))))))(input)?;
    is_not("= }\t\n\r")(input)
}
fn esc_value<'a>(input: &'a str) -> IResult<&'a str, &'a str> {
    let (input, (es, _)) = consumed(preceded(tag("\\"), anychar))(input)?;
    Ok((input, es))
}
fn value<'a>(input: &'a str) -> IResult<&'a str, &'a str> {
    // is_not(" }\t\n\r")(input)
    let (input, (v, _)) = consumed(many1(alt((esc_value, is_not(" }\t\n\r\\")))))(input)?;
    Ok((input, v))
}
fn attributes<'a>(input: &'a str) -> IResult<&'a str, HashMap<&'a str, &'a str>> {
    let (input, kvs) = delimited(
        tuple((tag("{"), space0)),
        separated_list1(
            tuple((opt(line_ending), space1)),
            separated_pair(key, tag("="), value),
        ),
        tuple((space0, tag("}"))),
    )(input)?;
    let mut h = HashMap::new();
    for (k, v) in kvs {
        // For security reasons only add first value of a key found.
        h.entry(k).or_insert(v);
    }
    Ok((input, h))
}

// SPANS

static SPANS: phf::Map<char, &'static str> = phf_map! {
    '*' => "Strong",
    '_' => "Emphasis",
    '^' => "Superscript",
    '~' => "Subscript",
    '#' => "Hash",
    '`' => "Verbatim",
    '=' => "Highlight",
    '+' => "Insert",
    '-' => "Delete",
};

// Strong      =  { "*" }
// Emphasis    =  { "_" }
// Superscript =  { "^" }
// Subscript   =  { "~" }
// Hash        =  { "#" }
// Verbatim    =  { "`"+ }
// Highlight   =  { "=" }
// Insert      =  { "+" }
// Delete      =  { "-" }
#[derive(Debug, PartialEq, Eq, Clone)]
pub enum Span<'a> {
    LineBreak(&'a str),
    NBWS(&'a str),
    Esc(&'a str),
    Text(&'a str),
    Hash(&'a str),
    EOM,
    // Tags with Attributes
    Link(&'a str, Vec<Span<'a>>, Option<HashMap<&'a str, &'a str>>),
    Verbatim(&'a str, Option<&'a str>, Option<HashMap<&'a str, &'a str>>),
    Strong(Vec<Span<'a>>, Option<HashMap<&'a str, &'a str>>),
    Emphasis(Vec<Span<'a>>, Option<HashMap<&'a str, &'a str>>),
    Superscript(Vec<Span<'a>>, Option<HashMap<&'a str, &'a str>>),
    Subscript(Vec<Span<'a>>, Option<HashMap<&'a str, &'a str>>),
    Highlight(Vec<Span<'a>>, Option<HashMap<&'a str, &'a str>>),
    Insert(Vec<Span<'a>>, Option<HashMap<&'a str, &'a str>>),
    Delete(Vec<Span<'a>>, Option<HashMap<&'a str, &'a str>>),
}

fn span_with_attributes<'a>(span: Span<'a>, kvs: HashMap<&'a str, &'a str>) -> Span<'a> {
    match span {
        // Tags with Attributes
        Span::Link(loc, ss, _) => Span::Link(loc, ss, Some(kvs)),
        Span::Verbatim(text, format, _) => Span::Verbatim(text, format, Some(kvs)),
        Span::Strong(ss, _) => Span::Strong(ss, Some(kvs)),
        Span::Emphasis(ss, _) => Span::Emphasis(ss, Some(kvs)),
        Span::Superscript(ss, _) => Span::Superscript(ss, Some(kvs)),
        Span::Subscript(ss, _) => Span::Subscript(ss, Some(kvs)),
        Span::Highlight(ss, _) => Span::Highlight(ss, Some(kvs)),
        Span::Insert(ss, _) => Span::Insert(ss, Some(kvs)),
        Span::Delete(ss, _) => Span::Delete(ss, Some(kvs)),
        Span::LineBreak(_)
        | Span::NBWS(_)
        | Span::Esc(_)
        | Span::Text(_)
        | Span::Hash(_)
        | Span::EOM => span,
    }
}

// End       =  { NEWLINE ~ NEWLINE | EOI }
fn eom<'a>(input: &'a str) -> IResult<&'a str, Span> {
    // Input has ended
    if input == "" {
        return Ok((input, Span::EOM));
    }
    // Common block terminator has ended
    // TODO: Account for whitespace and list indentations
    let (_i, _s) = tuple((line_ending, line_ending))(input)?;
    Ok((input, Span::EOM))
}

// LineBreak =  { "\\" ~ &NEWLINE }
fn esc<'a>(input: &'a str) -> IResult<&'a str, Span> {
    let (i, e) = preceded(
        tag("\\"),
        alt((tag(" "), line_ending, take_while_m_n(1, 1, |c| c != ' '))),
    )(input)?;
    if e == " " {
        Ok((i, Span::NBWS(e)))
    } else if let Ok(_) = line_ending::<_, ()>(e) {
        Ok((i, Span::LineBreak(e)))
    } else {
        Ok((i, Span::Esc(e)))
    }
}

// UnboundTag  = _{
//     Superscript
//   | Subscript
//   | Hash
//   | Verbatim
// }
// NOTE: Hash and Verbatim where handles separately.
fn nobracket<'a>(input: &'a str) -> IResult<&'a str, Span> {
    let (i, t) = alt((tag("^"), tag("~")))(input)?;
    let (i, ss) = spans(i, Some(&t), None)?;
    match t {
        "^" => Ok((i, Span::Superscript(ss, None))),
        "~" => Ok((i, Span::Subscript(ss, None))),
        _ => Err(nom::Err::Error(nom::error::Error::new(
            input,
            nom::error::ErrorKind::Alt,
        ))),
    }
}

// RawText   =  { (!(PEEK | NEWLINE) ~ ANY)+ }
// Raw       =  { PUSH(Verbatim) ~ RawText ~ (POP ~ Attribute* | &End ~ DROP) }
fn verbatim<'a>(input: &'a str) -> IResult<&'a str, Span> {
    let (input, svtag) = take_while1(|b| b == '`')(input)?;
    let mut char_total_length: usize = 0;
    let mut i = input;
    while i.len() > 0 {
        if let Ok((i, _evtag)) = tag::<_, &str, ()>("\n")(i) {
            let (content, _) = input.split_at(char_total_length);
            return Ok((i, Span::Verbatim(content, None, None)));
        } else if let Ok((ti, evtag)) = take_while1::<_, &str, ()>(|b| b == '`')(i) {
            if svtag == evtag {
                let (content, _) = input.split_at(char_total_length);
                // NOTE: May want to strip whitespace around enclosing backticks:
                //  `` `verbatim` `` -> <code>`verbatim`</code>
                let content_trimmed = content.trim();
                if content_trimmed.starts_with('`') && content_trimmed.ends_with('`') {
                    return Ok((ti, Span::Verbatim(content_trimmed, None, None)));
                }
                return Ok((ti, Span::Verbatim(content, None, None)));
            }
            i = ti;
            char_total_length += evtag.len();
        } else {
            let char_length = i.chars().next().unwrap().len_utf8();
            (_, i) = i.split_at(char_length);
            char_total_length += char_length;
        }
    }
    let (content, _) = input.split_at(char_total_length);
    Ok((i, Span::Verbatim(content, None, None)))
}

//{=format #identifier .class key=value key="value" %comment%}
// Field      = { ASCII_ALPHA ~ (ASCII_ALPHANUMERIC | "_")* }
fn field(input: &str) -> IResult<&str, &str> {
    is_not(" \t\r\n]")(input)
}
fn hash_field(input: &str) -> IResult<&str, &str> {
    let (input, (v, _)) = consumed(tuple((is_not(" \t\n\r]#"), opt(is_not("\t\r\n]#")))))(input)?;
    // NOTE: We may want to add any trailing spaces that where trimmed  back to input
    // as that could end up joining  words together that where seperated by closing tags.
    let v = v.trim();
    Ok((input, v))
}

// HashTag   =  { Edge ~ Hash ~ Location }
fn hash<'a>(input: &'a str) -> IResult<&'a str, Span> {
    let (i, h) = preceded(tag("#"), hash_field)(input)?;
    Ok((i, Span::Hash(h)))
}

// brackettag  = _{
//     edgetag    // strong(*), emphasis(_)
//   | highlight  // (=)
//   | insert     // (+)
//   | delete     // (-)
// }
// NOTE: Added for consistency the Superscript and Subscript
//   Span types to bracket tags for consistency and versatility.
fn bracket(input: &str) -> IResult<&str, Span> {
    let (i, t) = preceded(tag("["), is_a("*_=+-^~"))(input)?;
    let closing_tag = t.to_string() + "]";
    let (i, ss) = spans(i, Some(&closing_tag), None)?;
    match t {
        "*" => Ok((i, Span::Strong(ss, None))),
        "_" => Ok((i, Span::Emphasis(ss, None))),
        "=" => Ok((i, Span::Highlight(ss, None))),
        "+" => Ok((i, Span::Insert(ss, None))),
        "-" => Ok((i, Span::Delete(ss, None))),
        "^" => Ok((i, Span::Superscript(ss, None))),
        "~" => Ok((i, Span::Subscript(ss, None))),
        _ => Err(nom::Err::Error(nom::error::Error::new(
            input,
            nom::error::ErrorKind::Alt,
        ))),
    }
}

// Edge        =  { (" " | "\t")+ | NEWLINE | SOI | EOI }
// edgetag     = _{
//     strong
//   | emphasis
// }
fn at_boundary_end<'a>(closer: &'a str, input: &'a str) -> IResult<&'a str, &'a str> {
    terminated(
        tag(closer),
        alt((
            tag(" "),
            tag("\t"),
            tag("\n"),
            tag("\r"),
            tag("*"),
            tag("_"),
            tag("=]"),
            tag("+]"),
            tag("-]"),
            tag("^"),
            tag("~"),
            tag("]]"),
            tag("{"),
        )),
    )(input)
}
fn edge(input: &str) -> IResult<&str, Span> {
    let (i, t) = alt((tag("*"), tag("_")))(input)?;
    let _ = not(alt((tag(" "), tag("\n"), tag("\t"), tag("\r"))))(i)?;
    let (i, ss) = spans(i, Some(&t), None)?;
    match t {
        "*" => Ok((i, Span::Strong(ss, None))),
        "_" => Ok((i, Span::Emphasis(ss, None))),
        _ => Err(nom::Err::Error(nom::error::Error::new(
            input,
            nom::error::ErrorKind::Alt,
        ))),
    }
}

// LinkDlmr  = _{ "|" | &("]" | NEWLINE | EOI) }
// Locator   =  { (("\\" | !LinkDlmr) ~ ANY)+ }
fn locator(input: &str) -> IResult<&str, &str> {
    let (i, l) = is_not("|]")(input)?;
    let (i, _) = opt(tag("|"))(i)?;
    Ok((i, l))
}

// Link      =  { "[[" ~ Locator ~ LinkDlmr? ~ (!"]]" ~ (Span | Char))* ~ ("]]" ~ Attribute* | &End) }
fn link<'a>(input: &'a str) -> IResult<&'a str, Span> {
    let (i, l) = preceded(tag("[["), locator)(input)?;
    let (i, ss) = spans(i, Some("]]"), None)?;
    Ok((i, Span::Link(l, ss, None)))
}

// Char      =  { !NEWLINE ~ "\\"? ~ ANY }
// Span      =  {
//     Break
//   | Raw
//   | HashTag
//   | Link
//   | "[" ~ PUSH(BracketTag) ~ (!(PEEK ~ "]") ~ (Span | Char))+ ~ (POP ~ "]" ~ Attribute* | &End ~ DROP)
//   | PUSH(UnboundTag) ~ (!PEEK ~ (Span | Char))+ ~ (POP | &End ~ DROP)
//   | Edge ~ PUSH(EdgeTag) ~ (!(PEEK ~ Edge) ~ (Span | Char))+ ~ (POP ~ &Edge | &End ~ DROP)
//   | Char
//   | NEWLINE ~ !NEWLINE
// }
// TODO: Look at turn spans function signature
//   from (input: &str, closer: Option<&str>)
//     to (input: &str, closer: &str)
//   where starting closer as "" happens to also be the same as eof/eom
fn spans<'a, 'b>(
    input: &'a str,
    closer: Option<&'b str>,
    inlist: Option<bool>,
) -> IResult<&'a str, Vec<Span<'a>>> {
    let mut ss = Vec::new();
    let mut i = input;
    // Loop through text until reach two newlines
    // or in future matching valid list item.
    let mut boundary = true;
    let mut text_start = input;
    let mut char_total_length: usize = 0;
    let mut trim_closer = false;
    while i != "" {
        // println!(
        //     "input: {:?}\n  boundary: {:?}\n  closer: {:?}\n  text_start: {:?}",
        //     i, boundary, closer, text_start
        // );
        // if we just started or next char is boundary
        if let Some(closer) = closer {
            if i.starts_with(closer) {
                if boundary == false && (closer == "*" || closer == "_") {
                    if let Ok(_) = at_boundary_end(closer, i) {
                        trim_closer = true;
                        break;
                    }
                } else {
                    trim_closer = true;
                    break;
                }
            }
        }
        // Escape loop if notice any list starting tags
        if let Some(new_list) = inlist {
            if let Ok((_, true)) = is_list_singleline_tag(new_list, i) {
                break;
            }
        }
        // Automatically collect breaks and escaped char
        // and turn escaped spaces into non-breaking spaces
        // before checking for qwikmark tags
        if let (true, Ok((input, s))) = (boundary, edge(i)) {
            boundary = false;
            if char_total_length > 0 {
                let (text, _) = text_start.split_at(char_total_length);
                ss.push(Span::Text(text));
                char_total_length = 0;
            }
            text_start = input;
            i = input;
            ss.push(s);
        } else if let Ok((input, s)) = alt((eom, esc, verbatim, hash, link, bracket, nobracket))(i)
        {
            boundary = false;
            if char_total_length > 0 {
                let (text, _) = text_start.split_at(char_total_length);
                ss.push(Span::Text(text));
                char_total_length = 0;
            }
            text_start = input;
            i = input;
            // End of Mark (EOM) Indicates a common ending point
            // such as an end to a block such as a paragraph or
            // that the file input as ended.
            match s {
                Span::EOM => break,
                Span::Hash(_) | Span::Esc(_) => ss.push(s),
                _ => {
                    if let Ok((input, kvs)) = attributes(i) {
                        let s = span_with_attributes(s, kvs);
                        ss.push(s);
                        text_start = input;
                        i = input;
                    } else if let Span::Verbatim(content, _, _) = s {
                        if let Ok((input, format)) = preceded(tag("="), key)(i) {
                            ss.push(Span::Verbatim(content, Some(format), None));
                            text_start = input;
                            i = input;
                        } else {
                            ss.push(s);
                        }
                    } else {
                        ss.push(s);
                    }
                }
            }
        } else {
            let c = i.chars().next().unwrap();
            boundary = if c == ' ' || c == '\n' || c == '\t' || c == '\r' {
                true
            } else {
                false
            };
            let char_length = c.len_utf8();
            (_, i) = i.split_at(char_length);
            char_total_length += char_length;
        }
    }
    if char_total_length > 0 {
        let (text, i) = text_start.split_at(char_total_length);
        ss.push(Span::Text(text));
        text_start = i;
    }
    if trim_closer {
        if let Some(closer) = closer {
            (_, text_start) = text_start.split_at(closer.len());
        }
    }
    Ok((text_start, ss))
}

// Contents concatenates the text within the nested vectors of spans
pub fn contents<'a>(outer: Vec<Span<'a>>) -> Vec<&'a str> {
    outer
        .into_iter()
        .fold(vec![], |mut unrolled, result| -> Vec<&'a str> {
            let rs = match result {
                Span::Text(t) | Span::Verbatim(t, _, _) | Span::Hash(t) => vec![t],
                Span::Link(s, vs, _) => {
                    if vs.len() > 0 {
                        contents(vs)
                    } else {
                        vec![s]
                    }
                }
                Span::Strong(vs, _)
                | Span::Emphasis(vs, _)
                | Span::Superscript(vs, _)
                | Span::Subscript(vs, _)
                | Span::Highlight(vs, _)
                | Span::Insert(vs, _)
                | Span::Delete(vs, _) => contents(vs),
                Span::LineBreak(s) | Span::NBWS(s) | Span::Esc(s) => vec![s],
                Span::EOM => vec![],
            };
            unrolled.extend(rs);
            unrolled
        })
}

// LineHash = { Edge ~ Hash ~ Location }
// LineChar = { !("|" | NEWLINE) ~ "\\"? ~ ANY }
// LineEnd  = { "|" | NEWLINE | EOI }
// LinkLine = { "[[" ~ Location ~ "|"? ~ (!"]]" ~ (Line | LineChar))+ ~ ("]]" ~ Attribute* | &LineEnd) }
// Line = {
//     Raw
//   | LineHash
//   | Link
//   | "[" ~ PUSH(BracketTag) ~ (!(PEEK ~ "]") ~ (Line | LineChar))+ ~ (POP ~ "]" ~ Attribute* | &LineEnd ~ DROP)
//   | PUSH(UnboundTag) ~ (!PEEK ~ (Line | LineChar))+ ~ (POP | &(LineEnd ~ DROP))
//   | Edge ~ PUSH(EdgeTag) ~ (!(PEEK ~ Edge) ~ (Line | LineChar))+ ~ (POP ~ &Edge | &LineEnd ~ DROP)
//   | LineChar
// }

// BLOCKS

// Block = {
//     Div
//   | Quote
//   | Heading
//   | Code
//   | ListHead
//   | Table
//   | Paragraph
// }
#[derive(Debug, PartialEq, Eq)]
pub enum Block<'a> {
    // Div(name, [Block])
    Div(&'a str, Vec<Block<'a>>),
    Quote(Vec<Block<'a>>),
    Heading(HLevel, Vec<Span<'a>>),
    // Code(format, [Span::Text])
    Code(Option<&'a str>, &'a str),
    List(Vec<ListItem<'a>>),
    Table(Vec<Span<'a>>, Option<Vec<Align>>, Vec<Vec<Span<'a>>>),
    Paragraph(Vec<Span<'a>>),
}

// H1      = { "#" }
// H2      = { "##" }
// H3      = { "###" }
// H4      = { "####" }
// H5      = { "#####" }
// H6      = { "######" }
static HLEVEL: phf::Map<&'static str, HLevel> = phf_map! {
    "#" => HLevel::H1,
    "##" => HLevel::H2,
    "###" => HLevel::H3,
    "####" => HLevel::H4,
    "#####" => HLevel::H5,
    "######" => HLevel::H6,
};

#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum HLevel {
    H1 = 1,
    H2,
    H3,
    H4,
    H5,
    H6,
}

// Heading = { (NEWLINE+ | SOI) ~ (H6 | H5 | H4 | H3 | H2 | H1) ~ (" " | "\t")+ ~ Location ~ ((LinkDlmr ~ Span+)? ~ &(NEWLINE | EOI)) }
fn heading<'a>(input: &'a str) -> IResult<&'a str, Block<'a>> {
    let (i, htag) = terminated(take_while_m_n(1, 6, |c| c == '#'), space1)(input)?;
    let level = HLEVEL.get(htag).unwrap();
    let (i, ss) = spans(i, None, Some(false))?;
    Ok((i, Block::Heading(*level, ss)))
}

// CodeStart = { (NEWLINE+ | SOI) ~ PUSH("`"{3, 6}) ~ Attribute* }
// CodeText  = { NEWLINE ~ (!NEWLINE ~ ANY)* }
// CodeStop  = { NEWLINE ~ POP }
// Code      = { CodeStart ~ (!CodeStop ~ CodeText)* ~ (CodeStop | &(NEWLINE | EOI)) }
fn code<'a>(input: &'a str) -> IResult<&'a str, Block<'a>> {
    let (input, sctag) = terminated(take_while_m_n(3, 16, |c| c == '`'), not(tag("`")))(input)?;
    let (input, format) = opt(field)(input)?;
    let (input, _) = opt(alt((line_ending, eof)))(input)?;
    let mut i = input;
    let mut char_total_length: usize = 0;
    while i.len() > 0 {
        if let Ok((i, _)) = tuple((
            line_ending,
            tag::<_, &str, ()>(sctag),
            alt((line_ending, eof)),
        ))(i)
        {
            let (content, _) = input.split_at(char_total_length);
            return Ok((i, Block::Code(format, content)));
        }
        let char_length = i.chars().next().unwrap().len_utf8();
        (_, i) = i.split_at(char_length);
        char_total_length += char_length;
    }
    let (content, _) = input.split_at(char_total_length);
    Ok((i, Block::Code(format, content)))
}

// RomanLower = { "i" | "v" | "x" | "l" | "c" | "d" | "m" }
// RomanUpper = { "I" | "V" | "X" | "L" | "C" | "D" | "M" }
#[derive(Debug, PartialEq, Eq)]
pub enum Enumerator<'a> {
    // is_alpha includes lower and upper cases along with Roman is_a("ivxlcdm") and is_a("IVXLCDM")
    Alpha(&'a str),
    Digit(&'a str),
}

#[derive(Debug, PartialEq, Eq)]
pub enum Index<'a> {
    // : <<Locator>>
    Definition(&'a str),
    // (e), e), e.
    Ordered(Enumerator<'a>),
    // - [ ]
    // contents may be a checkbox indicated with space or x,
    // or input field indicated with digit1 or ratio (digit1:digit1)
    Task(&'a str),
    // -, +, *
    Unordered(&'a str),
}

#[derive(Debug, PartialEq, Eq)]
pub struct ListItem<'a>(
    Index<'a>,
    // Block::Paragraph(Vec<Span<'a>>),
    Vec<Span<'a>>,
    // Block::List(Vec<ListItem<'a>>),
    Option<Block<'a>>,
);

// Definition = { ": " ~ Field }
fn definition<'a>(input: &'a str) -> IResult<&'a str, Index<'a>> {
    let (i, d) = preceded(tuple((tag(":"), space1)), not_line_ending)(input)?;
    Ok((i, Index::Definition(d)))
}

// Definition = { ": " ~ Field }
fn definition_simple<'a>(input: &'a str) -> IResult<&'a str, Index<'a>> {
    let (i, _d) = tag(":")(input)?;
    Ok((i, Index::Definition("")))
}

// Ordered    = { (ASCII_DIGIT+ | RomanLower+ | RomanUpper+ | ASCII_ALPHA_LOWER+ | ASCII_ALPHA_UPPER+) ~ ("." | ")") }
fn ordered<'a>(input: &'a str) -> IResult<&'a str, Index<'a>> {
    let (i, (stag, o, etag)) = tuple((
        opt(tag("(")),
        alt((alpha1, digit1)),
        alt((tag(")"), tag("."))),
    ))(input)?;
    if stag == Some("(") && etag != ")" {
        return Err(nom::Err::Error(nom::error::Error::new(
            input,
            nom::error::ErrorKind::Alt,
        )));
    }
    if is_digit(o.bytes().next().unwrap()) {
        Ok((i, Index::Ordered(Enumerator::Digit(o))))
    } else {
        Ok((i, Index::Ordered(Enumerator::Alpha(o))))
    }
}

fn ratio<'a>(input: &'a str) -> IResult<&'a str, &'a str> {
    let (i, (r, _)) = consumed(separated_pair(digit1, char(':'), digit1))(input)?;
    Ok((i, r))
}

fn task<'a>(input: &'a str) -> IResult<&'a str, Index<'a>> {
    let (i, t) = delimited(
        tag("- ["),
        alt((tag(" "), tag("x"), ratio, digit1)),
        tag("]"),
    )(input)?;
    Ok((i, Index::Task(t)))
}

// Unordered  = { "-" | "+" | "*" }
fn unordered<'a>(input: &'a str) -> IResult<&'a str, Index<'a>> {
    let (i, u) = alt((tag("*"), tag("-"), tag("+")))(input)?;
    Ok((i, Index::Unordered(u)))
}

fn list_tag<'a>(input: &'a str) -> IResult<&'a str, Option<(&'a str, Index<'a>)>> {
    if let (input, Some((_, d, (idx, _)))) = opt(tuple((
        many0(line_ending),
        space0,
        alt((
            tuple((task, space1)),
            tuple((unordered, space1)),
            tuple((ordered, space1)),
            tuple((definition, peek(line_ending))),
        )),
    )))(input)?
    {
        Ok((input, Some((d, idx))))
    } else {
        Ok((input, None))
    }
}

fn is_list_singleline_tag<'a>(new: bool, input: &'a str) -> IResult<&'a str, bool> {
    if let (input, Some((_, _, _idx, _))) = opt(tuple((
        line_ending,
        cond(new, space0),
        alt((task, unordered, ordered, definition_simple)),
        space1,
    )))(input)?
    {
        Ok((input, true))
    } else {
        Ok((input, false))
    }
}

// ListBlock  = {
//   NEWLINE+ ~
//   PEEK[..] ~ PUSH((" " | "\t")+) ~ (Unordered | Ordered | Definition)
//                                  ~ (" " | NEWLINE) ~ ListItem ~
//   (PEEK[..] ~ (Unordered | Ordered) ~ " " ~ ListItem)* ~
//   DROP
// }
fn list_block<'a>(
    input: &'a str,
    depth: &'a str,
    index: Index<'a>,
) -> IResult<&'a str, Option<Block<'a>>> {
    let mut lis = Vec::new();
    let mut i = input;
    let mut idx = index;
    loop {
        let (input, li) = list_item(i, depth, idx)?;
        i = input;
        lis.push(li);
        if let (input, Some((d, index))) = list_tag(i)? {
            if d != depth {
                break;
            }
            idx = index;
            i = input;
        } else {
            break;
        }
    }
    Ok((i, Some(Block::List(lis))))
}

fn nested_list_block<'a>(input: &'a str, depth: &'a str) -> IResult<&'a str, Option<Block<'a>>> {
    if let (i, Some((d, index))) = list_tag(input)? {
        if d.len() <= depth.len() {
            Ok((input, None))
        } else {
            list_block(i, d, index)
        }
    } else {
        Ok((input, None))
    }
}

// ListItem   = { Span+ ~ ListBlock* }
fn list_item<'a>(
    input: &'a str,
    depth: &'a str,
    index: Index<'a>,
) -> IResult<&'a str, ListItem<'a>> {
    // NOTE: Verify spans should not be able to fail. i.e. Make a test case for empty string ""
    // Or if needs to fail wrap in opt(spans...)
    let (input, ss) = spans(input, None, Some(true))?;
    let (input, slb) = nested_list_block(input, depth)?;
    Ok((input, ListItem(index, ss, slb)))
}

// ListHead   = { ((NEWLINE+ | SOI) ~ PEEK[..]
//                ~ (Unordered | Ordered | Definition)
//                ~ (" " | NEWLINE) ~ ListItem)+ }
fn list<'a>(input: &'a str) -> IResult<&'a str, Block<'a>> {
    if let (i, Some((d, index))) = list_tag(input)? {
        if d == "" {
            if let (i, Some(lb)) = list_block(i, d, index)? {
                return Ok((i, lb));
            }
        }
    }
    Err(nom::Err::Error(nom::error::Error::new(
        input,
        nom::error::ErrorKind::Alt,
    )))
}

// CellEnd      = _{ "|" | &(NEWLINE | EOI) }
// Cell         =  { "|" ~ Line+ }
// Row          =  { Cell+ ~ CellEnd }
// AlignRight   =  { "-"+ ~ ":" }
// AlignDefault =  { "-"+ }
// AlignCenter  =  { ":" ~ "-"+ ~ ":" }
// AlignLeft    =  { ":" ~ "-"+ }
// Layout       =  { ("|" ~ " "* ~ (AlignRight | AlignDefault | AlignCenter | AlignLeft) ~ " "*)+ ~ CellEnd }
// Table        =  {
//   (NEWLINE+ | SOI) ~ Row ~
//   NEWLINE ~ Layout ~
//   (NEWLINE ~ Row)*
// }
#[derive(Debug, PartialEq, Eq)]
pub enum Align {
    Right,   // --:
    Default, // ---
    Center,  // :-:
    Left,    // :--
}

// Paragraph = { (NEWLINE+ | SOI) ~ Span+ ~ &(NEWLINE | EOI) }
fn paragraph<'a>(input: &'a str) -> IResult<&'a str, Block<'a>> {
    let (i, ss) = spans(input, None, None)?;
    Ok((i, Block::Paragraph(ss)))
}

// Div = {
//   (NEWLINE+ | SOI) ~ ":::" ~ Attribute* ~ " " ~ Field ~ &(NEWLINE | EOI) ~
//   (!(NEWLINE+ ~ ":::") ~ Block)* ~
//   NEWLINE+ ~ (":::" ~ &(NEWLINE | EOI) | EOI)
// }
// Document = { Block* ~ NEWLINE* ~ EOI }
fn blocks<'a>(input: &'a str, div: Option<&'a str>) -> IResult<&'a str, Vec<Block<'a>>> {
    let mut bs = Vec::new();
    let mut i = input;
    loop {
        // WARN: utilizing multispace here would cause lists that start with spaces
        // to look like new lists that start right after a newline, so cannot greedy
        // consume newlines with spaces.
        (i, _) = many0(line_ending)(i)?;
        if let Some(name) = div {
            let (input, div_close) = opt(terminated(tag(":::"), peek(alt((line_ending, eof)))))(i)?;
            if div_close != None {
                return Ok((input, vec![Block::Div(name, bs)]));
            }
        }
        // Div open
        // let mut div_open: Option<;
        let (input, div_open) = opt(tuple((terminated(tag(":::"), space1), field)))(i)?;
        i = input;
        if let Some((_, name)) = div_open {
            let mut div_bs: Vec<Block<'a>>;
            (i, div_bs) = blocks(i, Some(name))?;
            if let Some(d) = div_bs.pop() {
                bs.push(d);
            }
        } else {
            let (input, b) = alt((code, heading, list, paragraph))(i)?;
            i = input;
            bs.push(b);
        }
        if i == "" {
            if let Some(name) = div {
                return Ok((i, vec![Block::Div(name, bs)]));
            }
            return Ok((i, bs));
        }
    }
}

// TODO: change tag's vector of strings to Hash types
struct Document<'a> {
    blocks: Vec<Block<'a>>,
    references: Option<HashMap<&'a str, Block<'a>>>,
    tags: Option<HashMap<&'a str, Vec<&'a str>>>,
}

pub fn document<'a>(input: &'a str) -> IResult<&'a str, Document<'a>> {
    let (i, bs) = blocks(input, None)?;
    Ok((
        i,
        Document {
            blocks: bs,
            references: None, // Some(HashMap::new()),
            tags: None,       // Some(HashMap::new()),
        },
    ))
}

// Document = { Block* ~ NEWLINE* ~ EOI }
pub fn ast<'a>(input: &'a str) -> IResult<&'a str, Vec<Block<'a>>> {
    let (i, bs) = blocks(input, None)?;
    Ok((i, bs))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_block_paragraph_text_line_break() {
        assert_eq!(
            ast("line\\\n"),
            Ok((
                "",
                vec![Block::Paragraph(vec![
                    Span::Text("line"),
                    Span::LineBreak("\n")
                ])]
            ))
        );
    }

    #[test]
    fn test_block_paragraph_nbsp() {
        assert_eq!(
            ast("left\\ right"),
            Ok((
                "",
                vec![Block::Paragraph(vec![
                    Span::Text("left"),
                    Span::NBWS(" "),
                    Span::Text("right")
                ])]
            ))
        );
    }

    #[test]
    fn test_block_paragraph_text_edge_text() {
        assert_eq!(
            ast("left *strong* right"),
            Ok((
                "",
                vec![Block::Paragraph(vec![
                    Span::Text("left "),
                    Span::Strong(vec![Span::Text("strong")], None),
                    Span::Text(" right")
                ])]
            ))
        );
    }

    #[test]
    fn test_block_paragraph_text_edge_textedge_text() {
        assert_eq!(
            ast("l _*s* e_ r"),
            Ok((
                "",
                vec![Block::Paragraph(vec![
                    Span::Text("l "),
                    Span::Emphasis(
                        vec![Span::Strong(vec![Span::Text("s")], None), Span::Text(" e")],
                        None
                    ),
                    Span::Text(" r")
                ])]
            ))
        );
    }

    #[test]
    fn test_block_paragraph_text_edgetext_textedge_text() {
        assert_eq!(
            ast("l _e1 *s* e2_ r"),
            Ok((
                "",
                vec![Block::Paragraph(vec![
                    Span::Text("l "),
                    Span::Emphasis(
                        vec![
                            Span::Text("e1 "),
                            Span::Strong(vec![Span::Text("s")], None),
                            Span::Text(" e2")
                        ],
                        None
                    ),
                    Span::Text(" r")
                ])]
            ))
        );
    }

    #[test]
    fn test_block_paragraph_text_edge_edge_text() {
        assert_eq!(
            ast("l _*s*_ r"),
            Ok((
                "",
                vec![Block::Paragraph(vec![
                    Span::Text("l "),
                    Span::Emphasis(vec![Span::Strong(vec![Span::Text("s")], None)], None),
                    Span::Text(" r")
                ])]
            ))
        );
    }

    #[test]
    fn test_block_paragraph_text_verbatim_text() {
        assert_eq!(
            ast("left ``verbatim``=fmt right"),
            Ok((
                "",
                vec![Block::Paragraph(vec![
                    Span::Text("left "),
                    Span::Verbatim("verbatim", Some("fmt"), None),
                    Span::Text(" right")
                ])]
            ))
        );
    }

    #[test]
    fn test_block_paragraph_text_verbatim_newline() {
        assert_eq!(
            ast("left ``verbatim\n right"),
            Ok((
                "",
                vec![Block::Paragraph(vec![
                    Span::Text("left "),
                    Span::Verbatim("verbatim", None, None),
                    Span::Text(" right")
                ])]
            ))
        );
    }

    #[test]
    fn test_block_paragraph_text_verbatim_with_nonmatching_backtick() {
        assert_eq!(
            ast("left ``ver```batim``{format=fmt} right"),
            Ok((
                "",
                vec![Block::Paragraph(vec![
                    Span::Text("left "),
                    Span::Verbatim(
                        "ver```batim",
                        None,
                        Some(HashMap::from([("format", "fmt")]))
                    ),
                    Span::Text(" right")
                ])]
            ))
        );
    }

    #[test]
    fn test_block_paragraph_text_verbatim_with_enclosing_backtick() {
        assert_eq!(
            ast("left `` `verbatim` `` right"),
            Ok((
                "",
                vec![Block::Paragraph(vec![
                    Span::Text("left "),
                    Span::Verbatim("`verbatim`", None, None),
                    Span::Text(" right")
                ])]
            ))
        );
    }

    #[test]
    fn test_block_paragraph_hash_empty_eom() {
        assert_eq!(
            ast("left #"),
            Ok(("", vec![Block::Paragraph(vec![Span::Text("left #")])]))
        );
    }

    #[test]
    fn test_block_paragraph_hash_empty_space() {
        assert_eq!(
            ast("left # "),
            Ok(("", vec![Block::Paragraph(vec![Span::Text("left # ")])]))
        );
    }

    #[test]
    fn test_block_paragraph_hash_field_eom() {
        assert_eq!(
            ast("left #hash"),
            Ok((
                "",
                vec![Block::Paragraph(vec![
                    Span::Text("left "),
                    Span::Hash("hash")
                ])]
            ))
        );
    }

    #[test]
    fn test_block_paragraph_hash_field_newline() {
        assert_eq!(
            ast("left #hash 1 \nnext line"),
            Ok((
                "",
                vec![Block::Paragraph(vec![
                    Span::Text("left "),
                    Span::Hash("hash 1"),
                    Span::Text("\nnext line")
                ])]
            ))
        );
    }

    #[test]
    fn test_block_paragraph_link_location() {
        assert_eq!(
            ast("left [[loc]]"),
            Ok((
                "",
                vec![Block::Paragraph(vec![
                    Span::Text("left "),
                    Span::Link("loc", vec![], None)
                ])]
            ))
        );
    }

    #[test]
    fn test_block_paragraph_link_with_location_and_text_super() {
        assert_eq!(
            ast("left [[loc|text^sup^]] right"),
            Ok((
                "",
                vec![Block::Paragraph(vec![
                    Span::Text("left "),
                    Span::Link(
                        "loc",
                        vec![
                            Span::Text("text"),
                            Span::Superscript(vec![Span::Text("sup")], None)
                        ],
                        None
                    ),
                    Span::Text(" right")
                ])]
            ))
        );
    }

    #[test]
    fn test_block_paragraph_link_with_location_and_span() {
        assert_eq!(
            ast("left [[loc|text `verbatim`]] right"),
            Ok((
                "",
                vec![Block::Paragraph(vec![
                    Span::Text("left "),
                    Span::Link(
                        "loc",
                        vec![Span::Text("text "), Span::Verbatim("verbatim", None, None)],
                        None
                    ),
                    Span::Text(" right")
                ])]
            ))
        );
    }

    #[test]
    fn test_block_paragraph_nested_spans() {
        assert_eq!(
            ast("text-left [*strong-left [_emphasis-center_]\t[+insert-left [^superscript-center^] insert-right+] strong-right*] text-right"),
            Ok((
                "",
                vec![Block::Paragraph(vec![
                  Span::Text("text-left "),
                  Span::Strong(vec![
                    Span::Text("strong-left "),
                    Span::Emphasis(vec![
                      Span::Text("emphasis-center")
                    ], None),
                    Span::Text("\t"),
                    Span::Insert(vec![
                      Span::Text("insert-left "),
                      Span::Superscript(vec![
                        Span::Text("superscript-center")
                      ], None),
                      Span::Text(" insert-right")
                    ], None),
                    Span::Text(" strong-right")
                  ], None),
                  Span::Text(" text-right")
                ])]
            ))
        );
    }

    #[test]
    fn test_block_paragraph_link_attributes() {
        let doc = ast("left [[loc]]{k1=v1 k_2=v_2}");
        assert_eq!(
            doc,
            Ok((
                "",
                vec![Block::Paragraph(vec![
                    Span::Text("left "),
                    Span::Link(
                        "loc",
                        vec![],
                        Some(HashMap::from([("k1", "v1",), ("k_2", "v_2")]))
                    )
                ])]
            ))
        );
    }

    #[test]
    fn test_block_paragraph_link_multiline_attributes() {
        let doc = ast("left [[loc]]{k1=v1\n               k_2=v_2}");
        assert_eq!(
            doc,
            Ok((
                "",
                vec![Block::Paragraph(vec![
                    Span::Text("left "),
                    Span::Link(
                        "loc",
                        vec![],
                        Some(HashMap::from([("k1", "v1",), ("k_2", "v_2")]))
                    )
                ])]
            ))
        );
    }

    #[test]
    fn test_block_paragraph_link_space_valued_attributes() {
        let doc = ast(r#"left [[loc]]{k1=v1 k_2=v\ 2}"#);
        assert_eq!(
            doc,
            Ok((
                "",
                vec![Block::Paragraph(vec![
                    Span::Text("left "),
                    Span::Link(
                        "loc",
                        vec![],
                        Some(HashMap::from([("k1", "v1",), ("k_2", r#"v\ 2"#)]))
                    )
                ])]
            ))
        );
    }

    #[test]
    fn test_block_header_field_paragraph() {
        assert_eq!(
            ast("## [*strong heading*]"),
            Ok((
                "",
                vec![Block::Heading(
                    HLevel::H2,
                    vec![Span::Strong(vec![Span::Text("strong heading")], None)]
                )]
            ))
        );
    }

    #[test]
    fn test_block_header_field_paragraph_starting_text() {
        assert_eq!(
            ast("## header\nnext line [*strong*]\n\nnew paragraph"),
            Ok((
                "",
                vec![
                    Block::Heading(
                        HLevel::H2,
                        vec![
                            Span::Text("header\nnext line "),
                            Span::Strong(vec![Span::Text("strong")], None)
                        ]
                    ),
                    Block::Paragraph(vec![Span::Text("new paragraph")])
                ]
            ))
        );
    }

    #[test]
    fn test_block_div_w_para_in_div_w_heading() {
        assert_eq!(
            ast("::: div1\n\n## [*strong heading*]\n\n::: div2\n\n  line"),
            Ok((
                "",
                vec![Block::Div(
                    "div1",
                    vec![
                        Block::Heading(
                            HLevel::H2,
                            vec![Span::Strong(vec![Span::Text("strong heading")], None)]
                        ),
                        Block::Div("div2", vec![Block::Paragraph(vec![Span::Text("  line")])])
                    ]
                )]
            ))
        );
    }

    #[test]
    fn test_block_div_w_code() {
        assert_eq!(
            ast("::: div1\n\n```code\nline1\n````\nline3\n```\n\n:::"),
            Ok((
                "",
                vec![Block::Div(
                    "div1",
                    vec![Block::Code(Some("code"), "line1\n````\nline3")]
                )]
            ))
        );
    }

    #[test]
    fn test_block_unordered_list() {
        assert_eq!(
            ast("- l1\n\n- l2\n\n  - l2,1\n\n  - l2,2\n\n    - l2,2,1\n\n  - l2,3\n\n- l3"),
            Ok((
                "",
                vec![Block::List(vec![
                    ListItem(Index::Unordered("-"), vec![Span::Text("l1")], None),
                    ListItem(
                        Index::Unordered("-"),
                        vec![Span::Text("l2")],
                        Some(Block::List(vec![
                            ListItem(Index::Unordered("-"), vec![Span::Text("l2,1")], None),
                            ListItem(
                                Index::Unordered("-"),
                                vec![Span::Text("l2,2")],
                                Some(Block::List(vec![ListItem(
                                    Index::Unordered("-"),
                                    vec![Span::Text("l2,2,1")],
                                    None
                                )]))
                            ),
                            ListItem(Index::Unordered("-"), vec![Span::Text("l2,3")], None)
                        ]))
                    ),
                    ListItem(Index::Unordered("-"), vec![Span::Text("l3")], None)
                ])]
            ))
        );
    }

    #[test]
    fn test_block_unordered_list_singleline() {
        assert_eq!(
            ast("- l1\n- l2\n  - l2,1\n  - l2,2\n    - l2,2,1\n  - l2,3\n- l3"),
            Ok((
                "",
                vec![Block::List(vec![
                    ListItem(Index::Unordered("-"), vec![Span::Text("l1")], None),
                    ListItem(
                        Index::Unordered("-"),
                        vec![Span::Text("l2")],
                        Some(Block::List(vec![
                            ListItem(Index::Unordered("-"), vec![Span::Text("l2,1")], None),
                            ListItem(
                                Index::Unordered("-"),
                                vec![Span::Text("l2,2")],
                                Some(Block::List(vec![ListItem(
                                    Index::Unordered("-"),
                                    vec![Span::Text("l2,2,1")],
                                    None
                                )]))
                            ),
                            ListItem(Index::Unordered("-"), vec![Span::Text("l2,3")], None)
                        ]))
                    ),
                    ListItem(Index::Unordered("-"), vec![Span::Text("l3")], None)
                ])]
            ))
        );
    }

    #[test]
    fn test_block_ordered_list() {
        assert_eq!(
            ast("a) l1\n\n(B) l2\n\n  1. l2,1"),
            Ok((
                "",
                vec![Block::List(vec![
                    ListItem(
                        Index::Ordered(Enumerator::Alpha("a")),
                        vec![Span::Text("l1")],
                        None
                    ),
                    ListItem(
                        Index::Ordered(Enumerator::Alpha("B")),
                        vec![Span::Text("l2")],
                        Some(Block::List(vec![ListItem(
                            Index::Ordered(Enumerator::Digit("1")),
                            vec![Span::Text("l2,1")],
                            None
                        )]))
                    )
                ])]
            ))
        )
    }

    #[test]
    fn test_block_definition_list() {
        assert_eq!(
            ast(": ab\n  alpha\n\n: 12\n  digit\n\n  : iv\n    roman"),
            Ok((
                "",
                vec![Block::List(vec![
                    ListItem(Index::Definition("ab"), vec![Span::Text("\n  alpha")], None),
                    ListItem(
                        Index::Definition("12"),
                        vec![Span::Text("\n  digit")],
                        Some(Block::List(vec![ListItem(
                            Index::Definition("iv"),
                            vec![Span::Text("\n    roman")],
                            None
                        )]))
                    )
                ])]
            ))
        )
    }

    #[test]
    fn test_block_header_sigleline_unordered_list() {
        assert_eq!(
            ast("## [*strong heading*]\n- l1\n- l2"),
            Ok((
                "",
                vec![
                    Block::Heading(
                        HLevel::H2,
                        vec![Span::Strong(vec![Span::Text("strong heading")], None)]
                    ),
                    Block::List(vec![
                        ListItem(Index::Unordered("-"), vec![Span::Text("l1")], None),
                        ListItem(Index::Unordered("-"), vec![Span::Text("l2")], None)
                    ])
                ]
            ))
        )
    }

    #[test]
    fn test_block_header_sigleline_span_not_list() {
        assert_eq!(
            ast("## [*strong heading\n  - l1*]\n  - l2"),
            Ok((
                "",
                vec![Block::Heading(
                    HLevel::H2,
                    vec![
                        Span::Strong(vec![Span::Text("strong heading\n  - l1")], None),
                        Span::Text("\n  - l2")
                    ]
                )]
            ))
        )
    }

    #[test]
    fn test_block_task_list() {
        assert_eq!(
            ast(": ab\n  - [ ] alpha"),
            Ok((
                "",
                vec![Block::List(vec![ListItem(
                    Index::Definition("ab"),
                    vec![],
                    Some(Block::List(vec![ListItem(
                        Index::Task(" "),
                        vec![Span::Text("alpha")],
                        None
                    )])),
                ),])]
            ))
        )
    }

    #[test]
    fn test_content_of_block_paragraph_link_with_location_and_span() {
        let doc = ast("left \\\n[[loc|text `v` [*a[_b_]*]]] right");
        assert_eq!(
            doc,
            Ok((
                "",
                vec![Block::Paragraph(vec![
                    Span::Text("left "),
                    Span::LineBreak("\n"),
                    Span::Link(
                        "loc",
                        vec![
                            Span::Text("text "),
                            Span::Verbatim("v", None, None),
                            Span::Text(" "),
                            Span::Strong(
                                vec![
                                    Span::Text("a"),
                                    Span::Emphasis(vec![Span::Text("b"),], None)
                                ],
                                None
                            )
                        ],
                        None
                    ),
                    Span::Text(" right")
                ])]
            ))
        );
        if let Ok(("", v)) = doc {
            if let Block::Paragraph(ss) = &v[0] {
                let ts = contents(ss.to_vec());
                assert_eq!(
                    ts,
                    vec!["left ", "\n", "text ", "v", " ", "a", "b", " right"]
                );
            } else {
                panic!("Not able to get span from paragragh within vector {:?}", v);
            }
        } else {
            panic!(
                "Not able to get vector of blocks from document ast {:?}",
                doc
            );
        }
    }
}
