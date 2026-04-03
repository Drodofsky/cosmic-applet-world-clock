// Copyright 2023 System76 <info@system76.com>
// SPDX-License-Identifier: GPL-3.0-only

use cosmic::{
    Element, Task, app,
    applet::cosmic_panel_config::PanelAnchor,
    iced::{
        Alignment, Length, Rectangle, Subscription,
        futures::{SinkExt, StreamExt, channel::mpsc},
        widget::{column, row},
    },
    iced_futures::stream,
    iced_widget::Column,
    widget::{Id, autosize, button, container, rectangle_tracker::*, space},
};
use jiff::Timestamp;
use logind_zbus::manager::ManagerProxy;
use std::hash::Hash;
use std::sync::LazyLock;
use tokio::time;

static AUTOSIZE_MAIN_ID: LazyLock<Id> = LazyLock::new(|| Id::new("autosize-main"));

pub struct Window {
    core: cosmic::app::Core,
    rectangle_tracker: Option<RectangleTracker<u32>>,
    rectangle: Rectangle,
}

#[derive(Debug, Clone)]
pub enum Message {
    Tick,
    Rectangle(RectangleUpdate<u32>),
}

impl Window {
    fn vertical_layout(&self) -> Element<'_, Message> {
        let elements = Vec::new();
        // todo: split using formatToParts when it is implemented
        // https://github.com/unicode-org/icu4x/issues/4936#issuecomment-2128812667
        let date_time_col = Column::with_children(elements)
            .align_x(Alignment::Center)
            .spacing(4);

        Element::from(
            column!(
                date_time_col,
                space::horizontal().width(Length::Fixed(
                    (self.core.applet.suggested_size(true).0
                        + 2 * self.core.applet.suggested_padding(true).1)
                        as f32
                ))
            )
            .align_x(Alignment::Center),
        )
    }

    fn horizontal_layout(&self) -> Element<'_, Message> {
        Element::from(
            row!(
                self.core.applet.text("Tokyo 18:43"),
                container(space::vertical().height(Length::Fixed(
                    (self.core.applet.suggested_size(true).1
                        + 2 * self.core.applet.suggested_padding(true).1)
                        as f32
                )))
            )
            .align_y(Alignment::Center),
        )
    }
}

impl cosmic::Application for Window {
    type Message = Message;
    type Executor = cosmic::SingleThreadExecutor;
    type Flags = ();
    const APP_ID: &str = "com.system76.CosmicAppletTime";

    fn init(core: app::Core, _flags: Self::Flags) -> (Self, app::Task<Self::Message>) {
        (
            Self {
                core,
                rectangle_tracker: None,
                rectangle: Rectangle::default(),
            },
            Task::none(),
        )
    }

    fn core(&self) -> &cosmic::app::Core {
        &self.core
    }

    fn core_mut(&mut self) -> &mut cosmic::app::Core {
        &mut self.core
    }

    fn style(&self) -> Option<cosmic::iced::theme::Style> {
        Some(cosmic::applet::style())
    }

    fn subscription(&self) -> Subscription<Message> {
        fn time_subscription() -> Subscription<Message> {
            struct Wrapper {
                id: &'static str,
            }
            impl Hash for Wrapper {
                fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
                    self.id.hash(state);
                }
            }
            Subscription::run_with(Wrapper { id: "time-sub" }, |Wrapper { id: _ }| {
                stream::channel(1, move |mut output: mpsc::Sender<Message>| async move {
                    // Mark this receiver's state as changed so that it always receives an initial
                    // update during the loop below
                    // This allows us to avoid duplicating code from the loop
                    let period = 1;
                    let mut timer = time::interval(time::Duration::from_secs(period));
                    timer.set_missed_tick_behavior(time::MissedTickBehavior::Skip);

                    loop {
                        timer.tick().await;
                        let _ = output.send(Message::Tick).await;
                        let current = Timestamp::now().as_second() as u64 % period;
                        if current != 0 {
                            timer.reset_after(time::Duration::from_secs(period - current));
                        }
                    }
                })
            })
        }

        // Update the time when waking from sleep
        async fn wake_from_sleep(output: &mut mpsc::Sender<Message>) -> zbus::Result<()> {
            let connection = zbus::Connection::system().await?;
            let proxy = ManagerProxy::new(&connection).await?;

            while let Some(property) = proxy.receive_prepare_for_sleep().await?.next().await {
                let waking = !property.args()?.start();
                if waking {
                    let _ = output.send(Message::Tick).await;
                }
            }
            Ok(())
        }

        fn wake_from_sleep_subscription() -> Subscription<Message> {
            Subscription::run_with("wake-from-suspend-sub", |_| {
                stream::channel(1, |mut output| async move {
                    if let Err(err) = wake_from_sleep(&mut output).await {
                        tracing::error!(?err, "Failed to subscribe to wake-from-sleep signal");
                    }
                })
            })
        }

        Subscription::batch([
            rectangle_tracker_subscription(0).map(|e| Message::Rectangle(e.1)),
            time_subscription(),
            wake_from_sleep_subscription(),
        ])
    }

    fn update(&mut self, message: Self::Message) -> app::Task<Self::Message> {
        match message {
            Message::Tick => Task::none(),
            Message::Rectangle(u) => {
                match u {
                    RectangleUpdate::Rectangle(r) => {
                        self.rectangle = r.1;
                    }
                    RectangleUpdate::Init(tracker) => {
                        self.rectangle_tracker = Some(tracker);
                    }
                }
                Task::none()
            }
        }
    }

    fn view(&self) -> Element<'_, Message> {
        let horizontal = matches!(
            self.core.applet.anchor,
            PanelAnchor::Top | PanelAnchor::Bottom
        );

        let button = button::custom(if horizontal {
            self.horizontal_layout()
        } else {
            self.vertical_layout()
        })
        .padding(if horizontal {
            [0, self.core.applet.suggested_padding(true).0]
        } else {
            [self.core.applet.suggested_padding(true).0, 0]
        })
        .class(cosmic::theme::Button::AppletIcon);

        autosize::autosize(
            if let Some(tracker) = self.rectangle_tracker.as_ref() {
                Element::from(tracker.container(0, button).ignore_bounds(true))
            } else {
                button.into()
            },
            AUTOSIZE_MAIN_ID.clone(),
        )
        .into()
    }
}
