use tracing::{Event, info};
use tracing_subscriber::fmt::{FmtContext, FormatEvent, FormattedFields};

pub const PIXEL_FORMAT: rust_vnc::PixelFormat = rust_vnc::PixelFormat {
    bits_per_pixel: 32,
    depth: 24,
    big_endian: false,
    true_colour: true,
    red_max: 255,
    green_max: 255,
    blue_max: 255,
    red_shift: 16,
    green_shift: 8,
    blue_shift: 0,
};

struct MyTracingFormatter;
impl<S, N> FormatEvent<S, N> for MyTracingFormatter
where
    S: tracing::Subscriber + for<'a> tracing_subscriber::registry::LookupSpan<'a>,
    N: for<'a> tracing_subscriber::fmt::FormatFields<'a> + 'static,
{
    fn format_event(&self, ctx: &FmtContext<'_, S, N>, mut writer: tracing_subscriber::fmt::format::Writer<'_>, event: &Event<'_>) -> std::fmt::Result {
        let metadata = event.metadata();
        write!(&mut writer, "{}:{} {} [{}] - ", metadata.file().unwrap_or("unknown"), metadata.line().unwrap_or(0), chrono::Local::now().format("%Y-%m-%dT%H:%M:%S"), metadata.level())?;
        if let Some(scope) = ctx.event_scope() {
            for span in scope.from_root() {
                write!(&mut writer, "{}", span.name())?;

                // `FormattedFields` is a formatted representation of the span's
                // fields, which is stored in its extensions by the `fmt` layer's
                // `new_span` method. The fields will have been formatted
                // by the same field formatter that's provided to the event
                // formatter in the `FmtContext`.
                let ext = span.extensions();
                let fields = &ext
                    .get::<FormattedFields<N>>()
                    .expect("will never be `None`");

                // Skip formatting the fields if the span had no fields.
                if !fields.is_empty() {
                    write!(&mut writer, "{{{}}}", fields)?;
                }
                write!(&mut writer, ": ")?;
            }
        };
        ctx.field_format().format_fields(writer.by_ref(), event)?;
        writeln!(writer)
    }
}
pub fn init_logger() {
    tracing_subscriber::fmt()
        .with_timer(tracing_subscriber::fmt::time::time())
        .with_line_number(true)
        .with_file(true)
        .event_format(MyTracingFormatter)
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .with_writer(std::io::stdout)
        .init();
    info!("logger initialized");
    
    // env_logger::Builder::from_default_env()
    //     .format(|buf, record| {
    //         writeln!(
    //             buf,
    //             "{}:{} {} [{}] - {}",
    //             record.file().unwrap_or("unknown"),
    //             record.line().unwrap_or(0),
    //             chrono::Local::now().format("%Y-%m-%dT%H:%M:%S"),
    //             record.level(),
    //             record.args()
    //         )
    //     })
    //     .init();
}