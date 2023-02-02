use std::convert::TryFrom;

use matrix_sdk::ruma::{events::tag::TagName, OwnedUserId};

use modalkit::{
    editing::base::OpenTarget,
    env::vim::command::{CommandContext, CommandDescription},
    input::commands::{CommandError, CommandResult, CommandStep},
    input::InputContext,
};

use crate::base::{
    DownloadFlags,
    IambAction,
    IambId,
    MessageAction,
    ProgramCommand,
    ProgramCommands,
    ProgramContext,
    RoomAction,
    RoomField,
    SendAction,
    VerifyAction,
};

type ProgContext = CommandContext<ProgramContext>;
type ProgResult = CommandResult<ProgramCommand>;

/// Convert strings the user types into a tag name.
fn tag_name(name: String) -> Result<TagName, CommandError> {
    let tag = match name.as_str() {
        "fav" | "favorite" | "favourite" | "m.favourite" => TagName::Favorite,
        "low" | "lowpriority" | "low_priority" | "low-priority" | "m.lowpriority" => {
            TagName::LowPriority
        },
        "servernotice" | "server_notice" | "server-notice" | "m.server_notice" => {
            TagName::ServerNotice
        },
        _ => {
            if let Ok(tag) = name.parse() {
                TagName::User(tag)
            } else {
                let msg = format!("Invalid user tag name: {name}");

                return Err(CommandError::Error(msg));
            }
        },
    };

    Ok(tag)
}

fn iamb_invite(desc: CommandDescription, ctx: &mut ProgContext) -> ProgResult {
    let args = desc.arg.strings()?;

    if args.is_empty() {
        return Err(CommandError::InvalidArgument);
    }

    let ract = match args[0].as_str() {
        "accept" => {
            if args.len() != 1 {
                return Err(CommandError::InvalidArgument);
            }

            RoomAction::InviteAccept
        },
        "reject" => {
            if args.len() != 1 {
                return Err(CommandError::InvalidArgument);
            }

            RoomAction::InviteReject
        },
        "send" => {
            if args.len() != 2 {
                return Err(CommandError::InvalidArgument);
            }

            if let Ok(user) = OwnedUserId::try_from(args[1].as_str()) {
                RoomAction::InviteSend(user)
            } else {
                let msg = format!("Invalid user identifier: {}", args[1]);
                let err = CommandError::Error(msg);

                return Err(err);
            }
        },
        _ => {
            return Err(CommandError::InvalidArgument);
        },
    };

    let iact = IambAction::from(ract);
    let step = CommandStep::Continue(iact.into(), ctx.context.take());

    return Ok(step);
}

fn iamb_verify(desc: CommandDescription, ctx: &mut ProgContext) -> ProgResult {
    let mut args = desc.arg.strings()?;

    match args.len() {
        0 => {
            let open = ctx.switch(OpenTarget::Application(IambId::VerifyList));
            let step = CommandStep::Continue(open, ctx.context.take());

            return Ok(step);
        },
        1 => {
            return Result::Err(CommandError::InvalidArgument);
        },
        2 => {
            let act = match args[0].as_str() {
                "accept" => VerifyAction::Accept,
                "cancel" => VerifyAction::Cancel,
                "confirm" => VerifyAction::Confirm,
                "mismatch" => VerifyAction::Mismatch,
                "request" => {
                    let iact = IambAction::VerifyRequest(args.remove(1));
                    let step = CommandStep::Continue(iact.into(), ctx.context.take());

                    return Ok(step);
                },
                _ => return Result::Err(CommandError::InvalidArgument),
            };

            let vact = IambAction::Verify(act, args.remove(1));
            let step = CommandStep::Continue(vact.into(), ctx.context.take());

            return Ok(step);
        },
        _ => {
            return Result::Err(CommandError::InvalidArgument);
        },
    }
}

fn iamb_dms(desc: CommandDescription, ctx: &mut ProgContext) -> ProgResult {
    if !desc.arg.text.is_empty() {
        return Result::Err(CommandError::InvalidArgument);
    }

    let open = ctx.switch(OpenTarget::Application(IambId::DirectList));
    let step = CommandStep::Continue(open, ctx.context.take());

    return Ok(step);
}

fn iamb_members(desc: CommandDescription, ctx: &mut ProgContext) -> ProgResult {
    if !desc.arg.text.is_empty() {
        return Result::Err(CommandError::InvalidArgument);
    }

    let open = IambAction::Room(RoomAction::Members(ctx.clone().into()));
    let step = CommandStep::Continue(open.into(), ctx.context.take());

    return Ok(step);
}

fn iamb_cancel(desc: CommandDescription, ctx: &mut ProgContext) -> ProgResult {
    if !desc.arg.text.is_empty() {
        return Result::Err(CommandError::InvalidArgument);
    }

    let ract = IambAction::from(MessageAction::Cancel);
    let step = CommandStep::Continue(ract.into(), ctx.context.take());

    return Ok(step);
}

fn iamb_edit(desc: CommandDescription, ctx: &mut ProgContext) -> ProgResult {
    if !desc.arg.text.is_empty() {
        return Result::Err(CommandError::InvalidArgument);
    }

    let ract = IambAction::from(MessageAction::Edit);
    let step = CommandStep::Continue(ract.into(), ctx.context.take());

    return Ok(step);
}

fn iamb_redact(desc: CommandDescription, ctx: &mut ProgContext) -> ProgResult {
    let args = desc.arg.strings()?;

    if args.len() > 1 {
        return Result::Err(CommandError::InvalidArgument);
    }

    let ract = IambAction::from(MessageAction::Redact(args.into_iter().next()));
    let step = CommandStep::Continue(ract.into(), ctx.context.take());

    return Ok(step);
}

fn iamb_reply(desc: CommandDescription, ctx: &mut ProgContext) -> ProgResult {
    if !desc.arg.text.is_empty() {
        return Result::Err(CommandError::InvalidArgument);
    }

    let ract = IambAction::from(MessageAction::Reply);
    let step = CommandStep::Continue(ract.into(), ctx.context.take());

    return Ok(step);
}

fn iamb_rooms(desc: CommandDescription, ctx: &mut ProgContext) -> ProgResult {
    if !desc.arg.text.is_empty() {
        return Result::Err(CommandError::InvalidArgument);
    }

    let open = ctx.switch(OpenTarget::Application(IambId::RoomList));
    let step = CommandStep::Continue(open, ctx.context.take());

    return Ok(step);
}

fn iamb_spaces(desc: CommandDescription, ctx: &mut ProgContext) -> ProgResult {
    if !desc.arg.text.is_empty() {
        return Result::Err(CommandError::InvalidArgument);
    }

    let open = ctx.switch(OpenTarget::Application(IambId::SpaceList));
    let step = CommandStep::Continue(open, ctx.context.take());

    return Ok(step);
}

fn iamb_welcome(desc: CommandDescription, ctx: &mut ProgContext) -> ProgResult {
    if !desc.arg.text.is_empty() {
        return Result::Err(CommandError::InvalidArgument);
    }

    let open = ctx.switch(OpenTarget::Application(IambId::Welcome));
    let step = CommandStep::Continue(open, ctx.context.take());

    return Ok(step);
}

fn iamb_join(desc: CommandDescription, ctx: &mut ProgContext) -> ProgResult {
    let mut args = desc.arg.filenames()?;

    if args.len() != 1 {
        return Result::Err(CommandError::InvalidArgument);
    }

    let open = ctx.switch(args.remove(0));
    let step = CommandStep::Continue(open, ctx.context.take());

    return Ok(step);
}

fn iamb_room(desc: CommandDescription, ctx: &mut ProgContext) -> ProgResult {
    let mut args = desc.arg.strings()?;

    if args.len() < 2 {
        return Result::Err(CommandError::InvalidArgument);
    }

    let field = args.remove(0);
    let action = args.remove(0);

    if args.len() > 1 {
        return Result::Err(CommandError::InvalidArgument);
    }

    let act: IambAction = match (field.as_str(), action.as_str(), args.pop()) {
        // :room name set <room-name>
        ("name", "set", Some(s)) => RoomAction::Set(RoomField::Name, s).into(),
        ("name", "set", None) => return Result::Err(CommandError::InvalidArgument),

        // :room name unset
        ("name", "unset", None) => RoomAction::Unset(RoomField::Name).into(),
        ("name", "unset", Some(_)) => return Result::Err(CommandError::InvalidArgument),

        // :room topic set <topic>
        ("topic", "set", Some(s)) => RoomAction::Set(RoomField::Topic, s).into(),
        ("topic", "set", None) => return Result::Err(CommandError::InvalidArgument),

        // :room topic unset
        ("topic", "unset", None) => RoomAction::Unset(RoomField::Topic).into(),
        ("topic", "unset", Some(_)) => return Result::Err(CommandError::InvalidArgument),

        // :room tag set <tag-name>
        ("tag", "set", Some(s)) => RoomAction::Set(RoomField::Tag(tag_name(s)?), "".into()).into(),
        ("tag", "set", None) => return Result::Err(CommandError::InvalidArgument),

        // :room tag unset <tag-name>
        ("tag", "unset", Some(s)) => RoomAction::Unset(RoomField::Tag(tag_name(s)?)).into(),
        ("tag", "unset", None) => return Result::Err(CommandError::InvalidArgument),

        _ => return Result::Err(CommandError::InvalidArgument),
    };

    let step = CommandStep::Continue(act.into(), ctx.context.take());

    return Ok(step);
}

fn iamb_upload(desc: CommandDescription, ctx: &mut ProgContext) -> ProgResult {
    let mut args = desc.arg.strings()?;

    let sact = SendAction::Upload(args.pop());
    let iact = IambAction::from(sact);
    let step = CommandStep::Continue(iact.into(), ctx.context.take());

    return Ok(step);
}

fn iamb_download(desc: CommandDescription, ctx: &mut ProgContext) -> ProgResult {
    let mut args = desc.arg.strings()?;

    if args.len() > 1 {
        return Result::Err(CommandError::InvalidArgument);
    }

    let mut flags = DownloadFlags::NONE;
    if desc.bang {
        flags |= DownloadFlags::FORCE;
    };
    let mact = MessageAction::Download(args.pop(), flags);
    let iact = IambAction::from(mact);
    let step = CommandStep::Continue(iact.into(), ctx.context.take());

    return Ok(step);
}

fn iamb_open(desc: CommandDescription, ctx: &mut ProgContext) -> ProgResult {
    let mut args = desc.arg.strings()?;

    if args.len() > 1 {
        return Result::Err(CommandError::InvalidArgument);
    }

    let mut flags = DownloadFlags::OPEN;
    if desc.bang {
        flags |= DownloadFlags::FORCE;
    };
    let mact = MessageAction::Download(args.pop(), flags);
    let iact = IambAction::from(mact);
    let step = CommandStep::Continue(iact.into(), ctx.context.take());

    return Ok(step);
}

fn add_iamb_commands(cmds: &mut ProgramCommands) {
    cmds.add_command(ProgramCommand { names: vec!["cancel".into()], f: iamb_cancel });
    cmds.add_command(ProgramCommand { names: vec!["dms".into()], f: iamb_dms });
    cmds.add_command(ProgramCommand { names: vec!["download".into()], f: iamb_download });
    cmds.add_command(ProgramCommand { names: vec!["open".into()], f: iamb_open });
    cmds.add_command(ProgramCommand { names: vec!["edit".into()], f: iamb_edit });
    cmds.add_command(ProgramCommand { names: vec!["invite".into()], f: iamb_invite });
    cmds.add_command(ProgramCommand { names: vec!["join".into()], f: iamb_join });
    cmds.add_command(ProgramCommand { names: vec!["members".into()], f: iamb_members });
    cmds.add_command(ProgramCommand { names: vec!["redact".into()], f: iamb_redact });
    cmds.add_command(ProgramCommand { names: vec!["reply".into()], f: iamb_reply });
    cmds.add_command(ProgramCommand { names: vec!["rooms".into()], f: iamb_rooms });
    cmds.add_command(ProgramCommand { names: vec!["room".into()], f: iamb_room });
    cmds.add_command(ProgramCommand { names: vec!["spaces".into()], f: iamb_spaces });
    cmds.add_command(ProgramCommand { names: vec!["upload".into()], f: iamb_upload });
    cmds.add_command(ProgramCommand { names: vec!["verify".into()], f: iamb_verify });
    cmds.add_command(ProgramCommand { names: vec!["welcome".into()], f: iamb_welcome });
}

pub fn setup_commands() -> ProgramCommands {
    let mut cmds = ProgramCommands::default();

    add_iamb_commands(&mut cmds);

    return cmds;
}

#[cfg(test)]
mod tests {
    use super::*;
    use matrix_sdk::ruma::user_id;
    use modalkit::editing::action::WindowAction;

    #[test]
    fn test_cmd_verify() {
        let mut cmds = setup_commands();
        let ctx = ProgramContext::default();

        let res = cmds.input_cmd(":verify", ctx.clone()).unwrap();
        let act = WindowAction::Switch(OpenTarget::Application(IambId::VerifyList));
        assert_eq!(res, vec![(act.into(), ctx.clone())]);

        let res = cmds.input_cmd(":verify request @user1:example.com", ctx.clone()).unwrap();
        let act = IambAction::VerifyRequest("@user1:example.com".into());
        assert_eq!(res, vec![(act.into(), ctx.clone())]);

        let res = cmds
            .input_cmd(":verify accept @user1:example.com/FOOBAR", ctx.clone())
            .unwrap();
        let act = IambAction::Verify(VerifyAction::Accept, "@user1:example.com/FOOBAR".into());
        assert_eq!(res, vec![(act.into(), ctx.clone())]);

        let res = cmds
            .input_cmd(":verify mismatch @user2:example.com/QUUXBAZ", ctx.clone())
            .unwrap();
        let act = IambAction::Verify(VerifyAction::Mismatch, "@user2:example.com/QUUXBAZ".into());
        assert_eq!(res, vec![(act.into(), ctx.clone())]);

        let res = cmds
            .input_cmd(":verify cancel @user3:example.com/MYDEVICE", ctx.clone())
            .unwrap();
        let act = IambAction::Verify(VerifyAction::Cancel, "@user3:example.com/MYDEVICE".into());
        assert_eq!(res, vec![(act.into(), ctx.clone())]);

        let res = cmds
            .input_cmd(":verify confirm @user4:example.com/GOODDEV", ctx.clone())
            .unwrap();
        let act = IambAction::Verify(VerifyAction::Confirm, "@user4:example.com/GOODDEV".into());
        assert_eq!(res, vec![(act.into(), ctx.clone())]);

        let res = cmds.input_cmd(":verify confirm", ctx.clone());
        assert_eq!(res, Err(CommandError::InvalidArgument));

        let res = cmds.input_cmd(":verify cancel @user4:example.com MYDEVICE", ctx.clone());
        assert_eq!(res, Err(CommandError::InvalidArgument));

        let res = cmds.input_cmd(":verify mismatch a b c d e f", ctx.clone());
        assert_eq!(res, Err(CommandError::InvalidArgument));
    }

    #[test]
    fn test_cmd_join() {
        let mut cmds = setup_commands();
        let ctx = ProgramContext::default();

        let res = cmds.input_cmd("join #foobar:example.com", ctx.clone()).unwrap();
        let act = WindowAction::Switch(OpenTarget::Name("#foobar:example.com".into()));
        assert_eq!(res, vec![(act.into(), ctx.clone())]);

        let res = cmds.input_cmd("join #", ctx.clone()).unwrap();
        let act = WindowAction::Switch(OpenTarget::Alternate);
        assert_eq!(res, vec![(act.into(), ctx.clone())]);

        let res = cmds.input_cmd("join", ctx.clone());
        assert_eq!(res, Err(CommandError::InvalidArgument));

        let res = cmds.input_cmd("join foo bar", ctx.clone());
        assert_eq!(res, Err(CommandError::InvalidArgument));
    }

    #[test]
    fn test_cmd_room_invalid() {
        let mut cmds = setup_commands();
        let ctx = ProgramContext::default();

        let res = cmds.input_cmd("room", ctx.clone());
        assert_eq!(res, Err(CommandError::InvalidArgument));

        let res = cmds.input_cmd("room foo", ctx.clone());
        assert_eq!(res, Err(CommandError::InvalidArgument));

        let res = cmds.input_cmd("room set topic", ctx.clone());
        assert_eq!(res, Err(CommandError::InvalidArgument));
    }

    #[test]
    fn test_cmd_room_topic_set() {
        let mut cmds = setup_commands();
        let ctx = ProgramContext::default();

        let res = cmds
            .input_cmd("room topic set \"Lots of fun discussion!\"", ctx.clone())
            .unwrap();
        let act = RoomAction::Set(RoomField::Topic, "Lots of fun discussion!".into());
        assert_eq!(res, vec![(act.into(), ctx.clone())]);

        let res = cmds
            .input_cmd("room topic set The\\ Discussion\\ Room", ctx.clone())
            .unwrap();
        let act = RoomAction::Set(RoomField::Topic, "The Discussion Room".into());
        assert_eq!(res, vec![(act.into(), ctx.clone())]);

        let res = cmds.input_cmd("room topic set Development", ctx.clone()).unwrap();
        let act = RoomAction::Set(RoomField::Topic, "Development".into());
        assert_eq!(res, vec![(act.into(), ctx.clone())]);

        let res = cmds.input_cmd("room topic", ctx.clone());
        assert_eq!(res, Err(CommandError::InvalidArgument));

        let res = cmds.input_cmd("room topic set", ctx.clone());
        assert_eq!(res, Err(CommandError::InvalidArgument));

        let res = cmds.input_cmd("room topic set A B C", ctx.clone());
        assert_eq!(res, Err(CommandError::InvalidArgument));
    }

    #[test]
    fn test_cmd_room_name_invalid() {
        let mut cmds = setup_commands();
        let ctx = ProgramContext::default();

        let res = cmds.input_cmd("room name", ctx.clone());
        assert_eq!(res, Err(CommandError::InvalidArgument));

        let res = cmds.input_cmd("room name foo", ctx.clone());
        assert_eq!(res, Err(CommandError::InvalidArgument));
    }

    #[test]
    fn test_cmd_room_name_set() {
        let mut cmds = setup_commands();
        let ctx = ProgramContext::default();

        let res = cmds.input_cmd("room name set Development", ctx.clone()).unwrap();
        let act = RoomAction::Set(RoomField::Name, "Development".into());
        assert_eq!(res, vec![(act.into(), ctx.clone())]);

        let res = cmds
            .input_cmd("room name set \"Application Development\"", ctx.clone())
            .unwrap();
        let act = RoomAction::Set(RoomField::Name, "Application Development".into());
        assert_eq!(res, vec![(act.into(), ctx.clone())]);

        let res = cmds.input_cmd("room name set", ctx.clone());
        assert_eq!(res, Err(CommandError::InvalidArgument));
    }

    #[test]
    fn test_cmd_room_name_unset() {
        let mut cmds = setup_commands();
        let ctx = ProgramContext::default();

        let res = cmds.input_cmd("room name unset", ctx.clone()).unwrap();
        let act = RoomAction::Unset(RoomField::Name);
        assert_eq!(res, vec![(act.into(), ctx.clone())]);

        let res = cmds.input_cmd("room name unset foo", ctx.clone());
        assert_eq!(res, Err(CommandError::InvalidArgument));
    }

    #[test]
    fn test_cmd_room_tag_set() {
        let mut cmds = setup_commands();
        let ctx = ProgramContext::default();

        let res = cmds.input_cmd("room tag set favourite", ctx.clone()).unwrap();
        let act = RoomAction::Set(RoomField::Tag(TagName::Favorite), "".into());
        assert_eq!(res, vec![(act.into(), ctx.clone())]);

        let res = cmds.input_cmd("room tag set favorite", ctx.clone()).unwrap();
        let act = RoomAction::Set(RoomField::Tag(TagName::Favorite), "".into());
        assert_eq!(res, vec![(act.into(), ctx.clone())]);

        let res = cmds.input_cmd("room tag set fav", ctx.clone()).unwrap();
        let act = RoomAction::Set(RoomField::Tag(TagName::Favorite), "".into());
        assert_eq!(res, vec![(act.into(), ctx.clone())]);

        let res = cmds.input_cmd("room tag set low_priority", ctx.clone()).unwrap();
        let act = RoomAction::Set(RoomField::Tag(TagName::LowPriority), "".into());
        assert_eq!(res, vec![(act.into(), ctx.clone())]);

        let res = cmds.input_cmd("room tag set low-priority", ctx.clone()).unwrap();
        let act = RoomAction::Set(RoomField::Tag(TagName::LowPriority), "".into());
        assert_eq!(res, vec![(act.into(), ctx.clone())]);

        let res = cmds.input_cmd("room tag set low", ctx.clone()).unwrap();
        let act = RoomAction::Set(RoomField::Tag(TagName::LowPriority), "".into());
        assert_eq!(res, vec![(act.into(), ctx.clone())]);

        let res = cmds.input_cmd("room tag set servernotice", ctx.clone()).unwrap();
        let act = RoomAction::Set(RoomField::Tag(TagName::ServerNotice), "".into());
        assert_eq!(res, vec![(act.into(), ctx.clone())]);

        let res = cmds.input_cmd("room tag set server_notice", ctx.clone()).unwrap();
        let act = RoomAction::Set(RoomField::Tag(TagName::ServerNotice), "".into());
        assert_eq!(res, vec![(act.into(), ctx.clone())]);

        let res = cmds.input_cmd("room tag set server_notice", ctx.clone()).unwrap();
        let act = RoomAction::Set(RoomField::Tag(TagName::ServerNotice), "".into());
        assert_eq!(res, vec![(act.into(), ctx.clone())]);

        let res = cmds.input_cmd("room tag set u.custom-tag", ctx.clone()).unwrap();
        let act = RoomAction::Set(
            RoomField::Tag(TagName::User("u.custom-tag".parse().unwrap())),
            "".into(),
        );
        assert_eq!(res, vec![(act.into(), ctx.clone())]);

        let res = cmds.input_cmd("room tag set u.irc", ctx.clone()).unwrap();
        let act =
            RoomAction::Set(RoomField::Tag(TagName::User("u.irc".parse().unwrap())), "".into());
        assert_eq!(res, vec![(act.into(), ctx.clone())]);

        let res = cmds.input_cmd("room tag", ctx.clone());
        assert_eq!(res, Err(CommandError::InvalidArgument));

        let res = cmds.input_cmd("room tag set", ctx.clone());
        assert_eq!(res, Err(CommandError::InvalidArgument));

        let res = cmds.input_cmd("room tag set unknown", ctx.clone());
        assert_eq!(res, Err(CommandError::Error("Invalid user tag name: unknown".into())));

        let res = cmds.input_cmd("room tag set needs-leading-u-dot", ctx.clone());
        assert_eq!(
            res,
            Err(CommandError::Error("Invalid user tag name: needs-leading-u-dot".into()))
        );
    }

    #[test]
    fn test_cmd_room_tag_unset() {
        let mut cmds = setup_commands();
        let ctx = ProgramContext::default();

        let res = cmds.input_cmd("room tag unset favourite", ctx.clone()).unwrap();
        let act = RoomAction::Unset(RoomField::Tag(TagName::Favorite));
        assert_eq!(res, vec![(act.into(), ctx.clone())]);

        let res = cmds.input_cmd("room tag unset favorite", ctx.clone()).unwrap();
        let act = RoomAction::Unset(RoomField::Tag(TagName::Favorite));
        assert_eq!(res, vec![(act.into(), ctx.clone())]);

        let res = cmds.input_cmd("room tag unset fav", ctx.clone()).unwrap();
        let act = RoomAction::Unset(RoomField::Tag(TagName::Favorite));
        assert_eq!(res, vec![(act.into(), ctx.clone())]);

        let res = cmds.input_cmd("room tag unset low_priority", ctx.clone()).unwrap();
        let act = RoomAction::Unset(RoomField::Tag(TagName::LowPriority));
        assert_eq!(res, vec![(act.into(), ctx.clone())]);

        let res = cmds.input_cmd("room tag unset low-priority", ctx.clone()).unwrap();
        let act = RoomAction::Unset(RoomField::Tag(TagName::LowPriority));
        assert_eq!(res, vec![(act.into(), ctx.clone())]);

        let res = cmds.input_cmd("room tag unset low", ctx.clone()).unwrap();
        let act = RoomAction::Unset(RoomField::Tag(TagName::LowPriority));
        assert_eq!(res, vec![(act.into(), ctx.clone())]);

        let res = cmds.input_cmd("room tag unset servernotice", ctx.clone()).unwrap();
        let act = RoomAction::Unset(RoomField::Tag(TagName::ServerNotice));
        assert_eq!(res, vec![(act.into(), ctx.clone())]);

        let res = cmds.input_cmd("room tag unset server_notice", ctx.clone()).unwrap();
        let act = RoomAction::Unset(RoomField::Tag(TagName::ServerNotice));
        assert_eq!(res, vec![(act.into(), ctx.clone())]);

        let res = cmds.input_cmd("room tag unset server_notice", ctx.clone()).unwrap();
        let act = RoomAction::Unset(RoomField::Tag(TagName::ServerNotice));
        assert_eq!(res, vec![(act.into(), ctx.clone())]);

        let res = cmds.input_cmd("room tag unset u.custom-tag", ctx.clone()).unwrap();
        let act = RoomAction::Unset(RoomField::Tag(TagName::User("u.custom-tag".parse().unwrap())));
        assert_eq!(res, vec![(act.into(), ctx.clone())]);

        let res = cmds.input_cmd("room tag unset u.irc", ctx.clone()).unwrap();
        let act = RoomAction::Unset(RoomField::Tag(TagName::User("u.irc".parse().unwrap())));
        assert_eq!(res, vec![(act.into(), ctx.clone())]);

        let res = cmds.input_cmd("room tag", ctx.clone());
        assert_eq!(res, Err(CommandError::InvalidArgument));

        let res = cmds.input_cmd("room tag set", ctx.clone());
        assert_eq!(res, Err(CommandError::InvalidArgument));

        let res = cmds.input_cmd("room tag unset unknown", ctx.clone());
        assert_eq!(res, Err(CommandError::Error("Invalid user tag name: unknown".into())));

        let res = cmds.input_cmd("room tag unset needs-leading-u-dot", ctx.clone());
        assert_eq!(
            res,
            Err(CommandError::Error("Invalid user tag name: needs-leading-u-dot".into()))
        );
    }

    #[test]
    fn test_cmd_invite() {
        let mut cmds = setup_commands();
        let ctx = ProgramContext::default();

        let res = cmds.input_cmd("invite accept", ctx.clone()).unwrap();
        let act = IambAction::Room(RoomAction::InviteAccept);
        assert_eq!(res, vec![(act.into(), ctx.clone())]);

        let res = cmds.input_cmd("invite reject", ctx.clone()).unwrap();
        let act = IambAction::Room(RoomAction::InviteReject);
        assert_eq!(res, vec![(act.into(), ctx.clone())]);

        let res = cmds.input_cmd("invite send @user:example.com", ctx.clone()).unwrap();
        let act =
            IambAction::Room(RoomAction::InviteSend(user_id!("@user:example.com").to_owned()));
        assert_eq!(res, vec![(act.into(), ctx.clone())]);

        let res = cmds.input_cmd("invite", ctx.clone());
        assert_eq!(res, Err(CommandError::InvalidArgument));

        let res = cmds.input_cmd("invite foo", ctx.clone());
        assert_eq!(res, Err(CommandError::InvalidArgument));

        let res = cmds.input_cmd("invite accept @user:example.com", ctx.clone());
        assert_eq!(res, Err(CommandError::InvalidArgument));

        let res = cmds.input_cmd("invite reject @user:example.com", ctx.clone());
        assert_eq!(res, Err(CommandError::InvalidArgument));

        let res = cmds.input_cmd("invite send", ctx.clone());
        assert_eq!(res, Err(CommandError::InvalidArgument));

        let res = cmds.input_cmd("invite @user:example.com", ctx.clone());
        assert_eq!(res, Err(CommandError::InvalidArgument));
    }

    #[test]
    fn test_cmd_redact() {
        let mut cmds = setup_commands();
        let ctx = ProgramContext::default();

        let res = cmds.input_cmd("redact", ctx.clone()).unwrap();
        let act = IambAction::Message(MessageAction::Redact(None));
        assert_eq!(res, vec![(act.into(), ctx.clone())]);

        let res = cmds.input_cmd("redact Removed", ctx.clone()).unwrap();
        let act = IambAction::Message(MessageAction::Redact(Some("Removed".into())));
        assert_eq!(res, vec![(act.into(), ctx.clone())]);

        let res = cmds.input_cmd("redact \"Removed\"", ctx.clone()).unwrap();
        let act = IambAction::Message(MessageAction::Redact(Some("Removed".into())));
        assert_eq!(res, vec![(act.into(), ctx.clone())]);

        let res = cmds.input_cmd("redact Removed Removed", ctx.clone());
        assert_eq!(res, Err(CommandError::InvalidArgument));
    }
}
