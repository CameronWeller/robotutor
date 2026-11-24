use std::{
    error::Error,
    io::{self, Write},
};
use tao::{
    dpi::LogicalSize,
    event::{Event, WindowEvent},
    event_loop::{ControlFlow, EventLoopBuilder},
    window::WindowBuilder,
};
use wry::{NewWindowResponse, WebContext, WebViewBuilder};

fn main() -> Result<(), Box<dyn Error>> {
    if std::env::args().nth(1).as_deref() == Some("--version") {
        println!("robotutor-courses {}", env!("CARGO_PKG_VERSION"));
        return Ok(());
    }
    let port: u16 = std::env::var("ROBOTUTOR_PORT")?.parse()?;
    let home = format!("http://127.0.0.1:{port}/");
    let events = EventLoopBuilder::<u64>::with_user_event().build();
    let proxy = events.create_proxy();
    let window = WindowBuilder::new()
        .with_title("Robotutor Courses")
        .with_inner_size(LogicalSize::new(1060., 760.))
        .build(&events)?;
    let base = home.clone();
    let data = std::env::var_os("ROBOTUTOR_WEB_DATA").ok_or("Missing course data directory")?;
    let mut context = WebContext::new(Some(data.into()));
    let _view = WebViewBuilder::new_with_web_context(&mut context)
        .with_url(home)
        .with_navigation_handler(move |uri| {
            if let Some(id) = uri
                .strip_prefix("robotutor://assignment/")
                .and_then(|s| s.parse::<u64>().ok())
            {
                let _ = proxy.send_event(id);
                return false;
            }
            uri.starts_with(&base)
        })
        .with_new_window_req_handler(|_, _| NewWindowResponse::Deny)
        .build(&window)?;
    events.run(move |event, _, flow| {
        *flow = ControlFlow::Wait;
        match event {
            Event::UserEvent(id) => {
                println!("{id}");
                let _ = io::stdout().flush();
                *flow = ControlFlow::Exit;
            }
            Event::WindowEvent {
                event: WindowEvent::CloseRequested,
                ..
            } => *flow = ControlFlow::Exit,
            _ => (),
        }
    })
}
