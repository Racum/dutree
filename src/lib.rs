extern crate getopts;
extern crate terminal_size;
extern crate regex;
extern crate unicode_width;

use getopts::Options;
use std::io;
use std::path::{Path, PathBuf};
use std::fs;
use terminal_size::{Width, Height, terminal_size};
use regex::Regex;
use std::os::linux::fs::MetadataExt;
use std::env;

const VERSTR    : &str = "v0.1.0";
const DEF_WIDTH : u16  = 80;

pub enum XResult<T,S> {
    XOk(T),
    XErr(S),
    XExit,
}
use XResult::{XOk, XExit, XErr};

struct Entry {
    name    : String,
    bytes   : u64,
    color   : Option<String>,
    last    : bool,
    entries : Option<Vec<Entry>>,
}

pub struct Config {
    paths       : Vec<PathBuf>,
    color_dict  : Vec<DictEntry>,
    depth       : u8,
    depth_flag  : bool,
    bytes_flag  : bool,
    usage_flag  : bool,
    hiddn_flag  : bool,
    ascii_flag  : bool,
    aggr        : u64,
    exclude     : Vec<String>,
}

struct DictEntry { key : String, val : String }

fn init_opts() -> Options {
    let mut options = Options::new();

    options.optflagopt( "d", "depth"    , "show directories up to depth N (def 1)", "DEPTH" );
    options.optflagopt( "a", "aggr"     , "aggregate smaller than N B/KiB/MiB/GiB (def 1M)", "N[KMG]");
    options.optflag(    "s", "summary"  , "equivalent to -da, or -d1 -a1M" );
    options.optflag(    "u", "usage"    , "report real disk usage instead of file size");
    options.optflag(    "b", "bytes"    , "print sizes in bytes" );
    options.optmulti(   "x", "exclude"  , "exclude matching files or directories", "NAME");
    options.optflag(    "H", "no-hidden", "exclude hidden files" );
    options.optflag(    "A", "ascii"    , "ASCII characters only, no colors" );
    options.optflag(    "h", "help"     , "show help"            );
    options.optflag(    "v", "version"  , "print version number" );
    options
}

impl Config {
    pub fn new() -> XResult<Config, String> {

        let args: Vec<String> = env::args().collect();
        let program = args[0].clone();

        let options = init_opts();
        let opt = match options.parse(&args[1..]) {
            Ok(m)    => m,
            Err(err) =>{ print_usage(&program, &options); return XErr( err.to_string() )},
        };

        if opt.opt_present("h") {
            print_usage(&program, &options);
            return XExit;
        }
        if opt.opt_present("v") {
            println!("dutree version {}", VERSTR);
            return XExit;
        };

        let color_dict = create_color_dict();

        let mut paths : Vec<PathBuf> = Vec::new();
        if opt.free.len() == 0 {
            let mut path = std::path::PathBuf::new();
            path.push( ".".to_string() );
            paths.push( path );
        } else {
            for opt in &opt.free {
                let mut path = std::path::PathBuf::new();
                path.push( &opt );
                paths.push( path );
            }
        }

        for p in &paths {
            if !p.exists() {
                return XErr( format!( "path {} doesn't exist", p.display() ) );
            }
        }

        let mut depth_flag = opt.opt_present("d");
        let depth_opt = opt.opt_str("d");
        let mut depth = depth_opt.unwrap_or("1".to_string()).parse().unwrap_or(1);

        let bytes_flag = opt.opt_present("b");
        let usage_flag = opt.opt_present("u");
        let hiddn_flag = opt.opt_present("H");
        let ascii_flag = opt.opt_present("A");

        let mut aggr = if opt.opt_present("a") {
            let aggr_opt = opt.opt_str("a");
            let aggr_val = aggr_opt.unwrap_or("1M".to_string());

            if !Regex::new(r"^\d+\D?$").unwrap().is_match( aggr_val.as_str() ){
                return XErr( format!( "invalid argument '{}'", aggr_val ) );
            }

            let unit = aggr_val.matches(char::is_alphabetic).next().unwrap_or("B");
            let num : Vec<&str> = aggr_val.matches(char::is_numeric).collect();
            let num : u64       = num.concat().parse().unwrap();

            let factor = match unit {
                "b" | "B" => 1024u64.pow(0),
                "k" | "K" => 1024u64.pow(1),
                "m" | "M" => 1024u64.pow(2),
                "g" | "G" => 1024u64.pow(3),
                "t" | "T" => 1024u64.pow(4),
                _         => 1024u64.pow(0),
            };
            num * factor
        } else {
            0
        };

        let exclude = opt.opt_strs("x");

        if opt.opt_present("s") {
            depth_flag = true;
            depth      = 1;
            aggr       = 1024u64.pow(2);
        }

        XOk( Config{ paths, color_dict, depth, depth_flag, bytes_flag, 
            usage_flag, hiddn_flag, ascii_flag,  aggr, exclude } )
    }
}

fn try_is_symlink( path : &Path ) -> bool {
    let metadata = path.symlink_metadata();
    metadata.is_ok() && metadata.unwrap().file_type().is_symlink()
}

fn file_name_from_path( path : &Path ) -> String {
    let mut abspath = std::env::current_dir().unwrap();
    abspath.push( path );

    // don't resolve links
    if !try_is_symlink( path ) {
        abspath = abspath.canonicalize().unwrap_or( abspath );
    }

    abspath.file_name().unwrap_or( std::ffi::OsStr::new( "/" ) )  // '/' has no filename
           .to_str().unwrap_or( "[invalid name]" ).to_string()
}

fn try_read_dir( path : &Path ) -> Option<fs::ReadDir> {
    if try_is_symlink( path ) { return None } // don't follow symlinks
    match path.read_dir() {
        Ok(dir_list) => Some(dir_list),
        Err(err)     => { 
            print_io_error( path, err );
            None
        },
    }
}

fn try_bytes_from_path( path : &Path, usage_flag : bool ) -> u64 {

    match path.symlink_metadata() {
        Ok(metadata) => if usage_flag { metadata.st_blocks()*512 } else { metadata.st_size() },
        Err(err)     => { 
            print_io_error( path, err );
            0
        },
    }
}

fn path_from_dentry( entry : Result<fs::DirEntry, io::Error> ) -> Option<std::path::PathBuf> {
    match entry {
        Ok(entry) => {
            Some( entry.path() )
        },
        Err(err)  => {
            eprintln!( "Couldn't read entry ({:?})", err.kind() );
            None
        },
    }
}

fn print_io_error( path: &Path, err: io::Error ) {
    eprintln!( "Couldn't read {} ({:?})", file_name_from_path( path ), err.kind() )
}

impl Entry {
    fn new( path: &Path, cfg : &Config, depth : u8 ) -> Entry {
        let name = file_name_from_path( path );

        // recursively create directory tree of entries up to depth
        let depth = if cfg.depth_flag { depth - 1 } else { 1 };

        let entries = if path.is_dir() && ( !cfg.depth_flag || depth > 0 ) {
            let mut aggr_bytes = 0;
            if let Some( dir_list ) = try_read_dir( path ) {
                let mut vec : Vec<Entry> = Vec::new();
                for entry in dir_list {
                    if let Some( path ) = path_from_dentry( entry ) {
                        let entry_name = &file_name_from_path(&path);
                        if cfg.exclude.iter().any( |p| entry_name == p ){ continue }
                        if cfg.hiddn_flag && &entry_name[..1] == "."    { continue }
                        let entry = Entry::new( &path.as_path(), cfg, depth );
                        if cfg.aggr > 0 && entry.bytes < cfg.aggr {
                            aggr_bytes += entry.bytes;
                        } else {
                            vec.push( entry );
                        }
                    }
                }
                vec.sort_unstable_by( |a, b| b.bytes.cmp( &a.bytes ) );
                if aggr_bytes > 0 {
                    vec.push( Entry { 
                        name: "<aggregated>".to_string(),
                        bytes: aggr_bytes,
                        color: None,
                        last : true,
                        entries: None,
                    } );
                }

                let len = vec.len();
                if len > 0 {
                    vec[len-1].last = true;
                }

                Some( vec )
            } else { None }
        } else { None };

        // calculate sizes
        let bytes = if let Some(ref entries) = entries {
            let mut total = try_bytes_from_path( path, cfg.usage_flag );
            for entry in entries {
                total += entry.bytes;
            }
            total
        } else {
            get_bytes( path, cfg.usage_flag )
        };

        // calculate color
        let color = if !cfg.ascii_flag {color_from_path(path, &cfg.color_dict)} else {None};

        Entry { name, bytes, color, last: false, entries }
    }

    fn print_entries( &self, open_parents : Vec<bool>, parent_vals : Vec<u64>, 
                      bytes_flag : bool, ascii_flag : bool, bar_width : usize, tree_name_width : usize ) {
        if let Some(ref entries) = self.entries {
            for entry in entries {
                let mut op    = open_parents.clone();
                let mut bytes = parent_vals.clone();
                bytes.push( entry.bytes );

                // make sure the name column has the right length
                let tree_width = (open_parents.len() + 1) * 3; // 3 chars per tree branch
                if tree_name_width >= tree_width {
                    let name_width  = tree_name_width - tree_width;
                    let length = unicode_width::UnicodeWidthStr::width(entry.name.as_str());

                    let mut name = entry.name.clone();
                    name.truncate( name_width );

                    // surround name by ANSII color escape sequences
                    if let Some( ref col_str ) = entry.color {
                        name.insert( 0, 'm' );
                        name.insert( 0, 0o33 as char );
                        name.insert( 1, '[' );
                        name.insert_str( 2, col_str );
                        name.push( 0o33 as char );
                        name.push_str( "[0m" );
                    }

                    if length < name_width {
                        (length..name_width).for_each( |_| name.push( ' ' ) );
                    }

                    // draw the tree
                    for open in &open_parents {
                        if   *open { print!( "   " ); } 
                        else       { print!( "│  " ); }
                    }
                    if   entry.last { print!( "└─ " ); op.push( true  ); }
                    else            { print!( "├─ " ); op.push( false ); }

                    // print it
                    println!( "{} {} {:>13}", 
                              name,
                              fmt_bar( &bytes, bar_width, ascii_flag ),
                              fmt_size_str( entry.bytes, bytes_flag ) );
                    if let Some(_) = entry.entries {
                        entry.print_entries( op, bytes, bytes_flag, ascii_flag,
                                             bar_width, tree_name_width );
                    }
                }
            }
        }
    }

    fn print( &self, bytes_flag : bool, ascii_flag : bool ) {

        // calculate plot widths
        let mut twidth = DEF_WIDTH; 
        let size = terminal_size();
        if let Some( ( Width(w), Height(_h) ) ) = size {
            twidth = w;
        } else {
            eprintln!("Unable to get terminal size");
        }
        let size_width      = 15;
        let var_width       = twidth - size_width;
        let bar_width       = var_width as usize * 75 / 100;
        let tree_name_width = var_width as usize * 25 / 100;

        // initalize
        let     open_parents : Vec<bool> = Vec::new();
        let mut parent_vals  : Vec<u64>  = Vec::new();
        parent_vals.push( self.bytes );

        // print
        println!( "[ {} {} ]", self.name, fmt_size_str( self.bytes, bytes_flag ) );
        self.print_entries( open_parents, parent_vals, bytes_flag, ascii_flag,
                            bar_width, tree_name_width );
    }
}

fn fmt_bar( bytes : &Vec<u64>, width : usize, ascii_flag : bool ) -> String {
    let width = width as u64 - 2 - 5; // not including bars and percentage

    let mut str = String::with_capacity( width as usize );
    str.push( '│' );

    let mut bytesi = bytes.iter();
    let mut total  = bytesi.next().unwrap();
    let mut part   = bytesi.next().unwrap();
    let mut bars   = ( part * width ) / total;
    let mut pos    = width - bars;

    let block_char = if ascii_flag { vec![ ' ', '#' ] } else { vec![ ' ', '░', '▒', '▓', '█' ] };
    let mut chr    = 0;
    let levels = bytes.len() - 1;

    for x in 0..width {
        if x > pos {
            total = part;
            part  = bytesi.next().unwrap_or(&0);
            bars  = ( part * bars ) / total;

            pos = width - bars;
            chr += 1;
            if chr == levels || chr >= block_char.len() {
                chr = block_char.len() - 1;          // last level, solid '█'
            }
        }
        str.push( block_char[chr] );
    }

    format!( "{}│ {:3}%", str, ( bytes[bytes.len()-1] * 100 ) / bytes[bytes.len()-2] )
}

fn fmt_size_str( bytes : u64, flag : bool ) -> String {
    let b = bytes as f32;
    if      bytes < 1024 || flag   { format!( "{:.2} B"  , bytes                    ) }
    else if bytes < 1024u64.pow(2) { format!( "{:.2} KiB", b/1024.0                 ) }
    else if bytes < 1024u64.pow(3) { format!( "{:.2} MiB", b/(1024u32.pow(2) as f32)) }
    else if bytes < 1024u64.pow(4) { format!( "{:.2} GiB", b/(1024u32.pow(3) as f32)) }
    else                           { format!( "{:.2} TiB", b/(1024u32.pow(4) as f32)) }
}

fn get_bytes( path: &Path, usage_flag : bool ) -> u64 {
    if path.is_dir() {
        let mut bytes : u64 = try_bytes_from_path( path, usage_flag );
        if let Some(dir_list) = try_read_dir( path ) {
            for entry in dir_list {
                if let Some(path) = path_from_dentry( entry ) {
                    bytes += get_bytes( &path, usage_flag );
                }
            }
        }
        bytes
    } else {
        try_bytes_from_path( path, usage_flag )
    }
}

fn color_from_path( path : &Path, color_dict : &Vec<DictEntry> ) -> Option<String> {
    if try_is_symlink( path ) {
        if path.read_link().unwrap().exists() {
            if let Some( col ) = dict_get( color_dict, "ln" ) {
                return Some( col.to_string() );
            }
        } else {
            if let Some( col ) = dict_get( color_dict, "or" ) {
                return Some( col.to_string() );
            }
        }
    }
    let metadata = path.symlink_metadata();
    if metadata.is_ok() {
        let mode = metadata.unwrap().st_mode();
        if path.is_dir() {
            if mode & 0o002 != 0 {  // dir other writable
                if let Some( col ) = dict_get( color_dict, "ow" ) {
                    return Some( col.to_string() );
                }
            }
            if let Some( col ) = dict_get( color_dict, "di" ) {
                return Some( col.to_string() );
            }
        }
        if mode & 0o111 != 0 {  // executable
            if let Some( col ) = dict_get( color_dict, "ex" ) {
                return Some( col.to_string() );
            }
        }
    }
    if let Some( ext_str ) = path.extension() {
        for col in color_dict {
            if &col.key[..2] != "*." { continue }
            let key = col.key.trim_left_matches( "*." );
            if ext_str == key {
                return Some( dict_get( color_dict, col.key.as_str() ).unwrap().to_string() );
            }
        }
    }
    if path.is_file() {
        if let Some( col ) = dict_get( color_dict, "fi" ) {
            return Some( col.to_string() );
        }
        else { return None }
    }
    // we are assuming it can only be a 'bd','cd'. can also be 'pi','so' or 'no'
    if let Some( col ) = dict_get( color_dict, "bd" ) {
        return Some( col.to_string() );
    }
    None
}

fn print_usage( program: &str, opts: &Options ) {
    let brief = format!( "Usage: {} [options] <path> [<path>..]", program );
    print!( "{}", opts.usage( &brief ) );
}

fn dict_get( dict : &Vec<DictEntry>, key : &str ) -> Option<String> {
    for entry in dict {
        if &entry.key == key {
            return Some( entry.val.clone() );
        }
    }
    None
}

fn create_color_dict() -> Vec<DictEntry> {
    let mut color_dict = Vec::new();
    let env_str = env::var("LS_COLORS").unwrap_or( "".to_string() );
    for entry in env_str.split(':') {
        if entry.len() == 0 { break; }

        let     line = entry.replace("\"","");
        let mut line = line.split('=');
        let key      = line.next().unwrap();
        let val      = line.next().unwrap();

        color_dict.push( DictEntry{ key: key.to_string(), val: val.to_string() } );
    }
    color_dict
}

pub fn run( cfg: &Config ) {
    let entry = if cfg.paths.len() == 1 {
        Entry::new( cfg.paths[0].as_path(), &cfg, cfg.depth + 1 )
    } else {
        let mut bytes = 0;
        let mut entries : Vec<Entry> = Vec::new();

        for path in &cfg.paths {
            let e = Entry::new( path.as_path(), &cfg, cfg.depth + 1 );
            bytes += e.bytes;
            entries.push( e );
        }
        let len = entries.len();
        if len > 0 {
            entries[len-1].last = true;
        }
        Entry { 
            name    : "<collection>".to_string(),
            bytes,
            color   : None,
            last    : false,
            entries : Some(entries)
        }
    };

    entry.print( cfg.bytes_flag, cfg.ascii_flag );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_ls_colors() {
        let mut dict = Vec::new();
        dict.push( DictEntry{ key: "di".to_string()   , val: "dircode".to_string() } );
        dict.push( DictEntry{ key: "li".to_string()   , val: "linkcod".to_string() } );
        dict.push( DictEntry{ key: "*.mp3".to_string(), val: "mp3code".to_string() } );
        dict.push( DictEntry{ key: "*.tar".to_string(), val: "tarcode".to_string() } );
        assert_eq!( "dircode", color_from_path( Path::new(".")       , &dict ) );
        assert_eq!( "mp3code", color_from_path( Path::new("test.mp3"), &dict ) );
        assert_eq!( "tarcode", color_from_path( Path::new("test.tar"), &dict ) );
    }

    /*
    #[test]
    fn plot_bar() {
        println!("{}", fmt_bar(100, 100, 40 ) );
        println!("{}", fmt_bar( 90, 100, 40 ) );
        println!("{}", fmt_bar( 40, 100, 40 ) );
        println!("{}", fmt_bar( 30, 100, 40 ) );
        println!("{}", fmt_bar( 20, 100, 40 ) );
        println!("{}", fmt_bar( 10, 100, 40 ) );
        println!("{}", fmt_bar(  0, 100, 40 ) );
    }

    #[test]
    fn path_flavours() {
    // different paths, like . .. ../x /home/ dir / /usr/etc/../bin
    }

    #[test]
    fn entry_object() {
        let dir   = fs::read_dir(".").unwrap().nth(0).unwrap().unwrap();
        let entry = Entry::new(None, &dir, true, 0, false);
        println!( "entry created {} {}B", entry.name, entry.bytes );
        assert_eq!( ".git", entry.name );
    }

    #[test]
    fn vector_of_entries() {
        let mut vec : Vec<Entry> = Vec::new();
        let entry = Entry { name: String::from("file1"), bytes: 1, dir: None };
        vec.push( entry );
        let entry = Entry { name: String::from("file2"), bytes: 2, dir: None };
        vec.push( entry );
        vec.sort_unstable_by(|a, b| b.bytes.cmp(&a.bytes));
        assert!( vec[0].bytes == 2 );
    }

    #[test]
    fn get_bytes_test() {
        println!( "calculated bytes {}",
                  get_bytes( Path::new( "." ) ) );
        println!( "calculated bytes {}",
                  get_bytes( Path::new( "Cargo.toml" ) ) );
    }
    */
}