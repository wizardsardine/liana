use iced::Command;

use liana_ui::widget::Element;

use crate::{
    hw::HardwareWallets,
    installer::{
        context::DescriptorTemplate,
        message::Message,
        step::{Context, Step},
        view,
    },
};

pub struct ChooseDescriptorTemplate {
    template: DescriptorTemplate,
}

impl Default for ChooseDescriptorTemplate {
    fn default() -> Self {
        Self {
            template: DescriptorTemplate::Custom,
        }
    }
}

impl From<ChooseDescriptorTemplate> for Box<dyn Step> {
    fn from(s: ChooseDescriptorTemplate) -> Box<dyn Step> {
        Box::new(s)
    }
}
impl Step for ChooseDescriptorTemplate {
    fn update(&mut self, _hws: &mut HardwareWallets, message: Message) -> Command<Message> {
        if let Message::SelectDescriptorTemplate(template) = message {
            self.template = template;
            Command::perform(async move {}, |_| Message::Next)
        } else {
            Command::none()
        }
    }

    fn apply(&mut self, ctx: &mut Context) -> bool {
        ctx.descriptor_template = self.template;
        true
    }

    fn view<'a>(
        &'a self,
        _hws: &'a HardwareWallets,
        progress: (usize, usize),
        _email: Option<&'a str>,
    ) -> Element<Message> {
        view::editor::template::choose_descriptor_template(progress)
    }
}

pub struct DescriptorTemplateDescription {
    template: DescriptorTemplate,
}

impl Default for DescriptorTemplateDescription {
    fn default() -> Self {
        Self {
            template: DescriptorTemplate::Custom,
        }
    }
}

impl From<DescriptorTemplateDescription> for Box<dyn Step> {
    fn from(s: DescriptorTemplateDescription) -> Box<dyn Step> {
        Box::new(s)
    }
}

impl Step for DescriptorTemplateDescription {
    fn load_context(&mut self, ctx: &Context) {
        self.template = ctx.descriptor_template;
    }

    fn skip(&self, ctx: &Context) -> bool {
        ctx.descriptor_template == DescriptorTemplate::Custom
    }

    fn view<'a>(
        &'a self,
        _hws: &'a HardwareWallets,
        progress: (usize, usize),
        _email: Option<&'a str>,
    ) -> Element<Message> {
        view::editor::template::inheritance::inheritance_template_description(progress)
    }
}
