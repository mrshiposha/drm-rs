extern crate drm;
extern crate image;
extern crate rustyline;
extern crate nix;
extern crate passfd;

/// Check the `util` module to see how the `Card` structure is implemented.
pub mod utils;

use drm::buffer::DrmFourcc;
use passfd::FdPassingExt;

use drm::control::{from_u32, RawResourceHandle, DrmLeaseCreateResult, lease::LesseeId, connector, crtc};
use nix::fcntl::OFlag;
use rustyline::Editor;
use utils::*;
use std::{os::{unix::net::{UnixListener, UnixStream}, fd::RawFd}, path::Path, io::Read};


fn main() {
    // Using rustyline to create the interactive prompt.
    let editor_config = rustyline::config::Builder::new()
        .max_history_size(256)
        .completion_type(rustyline::config::CompletionType::List)
        .edit_mode(rustyline::config::EditMode::Vi)
        .auto_add_history(true)
        .build();
    let mut editor = rustyline::Editor::<()>::with_config(editor_config);

    let line = editor.readline("DRM `master` or `lessee <id>`? ").unwrap();
    let args: Vec<_> = line.split_whitespace().collect();
    match &args[..] {
        ["master"] => master(editor),
        ["lessee", id] => {
            let id: u32 = str::parse(id).unwrap();

            lessee(editor, id);
        },
        ["quit"] => (),
        ["help"] => {
            println!("master");
            println!("lessee <id>");
            println!("quit");
        },
        [] => (),
        _ => {
            println!("Unknown command");
        }
    }
}

fn master(mut editor: Editor<()>) {
    let card = Card::open_global();

    for line in editor.iter("Master> ").map(|x| x.unwrap()) {
        let args: Vec<_> = line.split_whitespace().collect();
        match &args[..] {
            ["ListConnectors"] => list_connectors(&card),
            ["CreateLease", resources @ ..] => {
                let resources = resources.iter()
                    .map(|handle| {
                        let handle = str::parse(handle).unwrap();
                        let handle: RawResourceHandle = from_u32(handle).unwrap();

                        handle
                    }).collect::<Vec<_>>();

                let DrmLeaseCreateResult {
                    fd,
                    lessee_id,
                } = card.create_lease(&resources, OFlag::O_CLOEXEC | OFlag::O_NONBLOCK).unwrap();

                println!("the lease sucessfully created");
                println!("lessee fd: {fd}; lessee id: {}", u32::from(lessee_id));
                println!("please, run the DRM lease with the appropriate lessee id");

                let socketpath = format!("/tmp/lessee-{}", u32::from(lessee_id));
                let socketpath = Path::new(&socketpath);

                if socketpath.try_exists().unwrap() {
                    std::fs::remove_file(&socketpath).unwrap();
                }

                let listener = UnixListener::bind(socketpath)
                    .unwrap();

                let (mut stream, _) = listener.accept().unwrap();
                stream.send_fd(fd).unwrap();

                let mut buf = [0; 1];
                let _ = stream.read(&mut buf);

                break;
            },
            ["ListLessees"] => println!("{:?}", card.list_lessees().unwrap()),
            ["GetLease", fd] => {
                let fd: RawFd = str::parse(fd).unwrap();

                let lessee_card = unsafe {
                    Card::open_fd(fd)
                };

                println!("{:?}", lessee_card.get_lease().unwrap());
            },
            ["RevokeLease", id] => {
                let lessee_id: u32 = str::parse(id).unwrap();
                let lessee_id: LesseeId = lessee_id.into();

                card.revoke_lease(lessee_id).unwrap();
            },
            ["help"] => {
                println!("ListConnectors");
                println!("CreateLease <handles...> // do not forget to lease a CRTC for a given connector");
                println!("ListLessees");
                println!("GetLease <lessee fd>");
                println!("RevokeLease <lessee id>");
                println!("quit");
            },
            ["quit"] => break,
            [] => (),
            _ => {
                println!("Unknown command");
            }
        }
    }

    
}

fn lessee(mut editor: Editor<()>, lessee_id: u32) {
    println!("trying to obtain the lessee #{lessee_id}'s fd...");

    let stream = UnixStream::connect(format!("/tmp/lessee-{}", lessee_id))
        .unwrap();
    let fd = stream.recv_fd().unwrap();

    let card = unsafe {
        Card::open_fd(fd)
    };

    println!("lessee's fd opened");

    for line in editor.iter(&format!("Lessee #{lessee_id}> ")).map(|x| x.unwrap()) {
        let args: Vec<_> = line.split_whitespace().collect();
        match &args[..] {
            ["ListConnectors"] => list_connectors(&card),
            ["GetLease"] => println!("{:?}", card.get_lease().unwrap()),
            ["ModeSet"] => modeset(&card),
            ["help"] => {
                println!("ListConnectors");
                println!("GetLease");
                println!("ModeSet");
                println!("quit");
            },
            ["quit"] => break,
            [] => (),
            _ => {
                println!("Unknown command");
            }
        }
    }
}

fn list_connectors(card: &Card) {
    let resources = card.resource_handles().unwrap();

    for connector_handle in resources.connectors() {
        let connector = card.get_connector(*connector_handle, false).unwrap();

        println!(
            "connector: {:?} ({}-{})",
            connector_handle,
            connector.interface().as_str(),
            connector.interface_id(),
        );

        println!("crtcs:");

        for encoder in connector.encoders() {
            let encoder = card.get_encoder(*encoder).unwrap();
            let crtc = encoder.crtc();

            if let Some(crtc_handle) = crtc {
                let crtc = card.get_crtc(crtc_handle).unwrap();
                println!("\t- {:?}, position: {:?}", crtc_handle, crtc.position());
            }
        }

        println!();
    }
}

fn modeset(card: &Card) {
    // Load the information.
    let res = card
        .resource_handles()
        .expect("Could not load normal resource ids.");
    let coninfo: Vec<connector::Info> = res
        .connectors()
        .iter()
        .flat_map(|con| card.get_connector(*con, true))
        .collect();
    let crtcinfo: Vec<crtc::Info> = res
        .crtcs()
        .iter()
        .flat_map(|crtc| card.get_crtc(*crtc))
        .collect();

    // Filter each connector until we find one that's connected.
    let con = coninfo
        .iter()
        .find(|&i| i.state() == connector::State::Connected)
        .expect("No connected connectors");

    // Get the first (usually best) mode
    let &mode = con.modes().get(0).expect("No modes found on connector");

    let (disp_width, disp_height) = mode.size();

    // Find a crtc and FB
    let crtc = crtcinfo.get(0).expect("No crtcs found");
    let old_fd = crtc.framebuffer().expect("framebuffer not found");

    // Select the pixel format
    let fmt = DrmFourcc::Xrgb8888;

    // Create a DB
    // If buffer resolution is larger than display resolution, an ENOSPC (not enough video memory)
    // error may occur
    let mut db = card
        .create_dumb_buffer((disp_width.into(), disp_height.into()), fmt, 32)
        .expect("Could not create dumb buffer");

    // Map it and grey it out.
    {
        let mut map = card
            .map_dumb_buffer(&mut db)
            .expect("Could not map dumbbuffer");
        for b in map.as_mut() {
            *b = 128;
        }
    }

    // Create an FB:
    let fb = card
        .add_framebuffer(&db, 24, 32)
        .expect("Could not create FB");

    println!("{:#?}", mode);
    println!("{:#?}", fb);
    println!("{:#?}", db);

    // Set the crtc
    // On many setups, this requires root access.
    card.set_crtc(crtc.handle(), Some(fb), (0, 0), &[con.handle()], Some(mode))
        .expect("Could not set CRTC");

    let five_seconds = ::std::time::Duration::from_millis(5000);
    ::std::thread::sleep(five_seconds);

    card.destroy_framebuffer(fb).unwrap();
    card.destroy_dumb_buffer(db).unwrap();

    card.set_crtc(crtc.handle(), Some(old_fd), (0, 0), &[con.handle()], Some(mode))
        .expect("Could not set CRTC");
}
