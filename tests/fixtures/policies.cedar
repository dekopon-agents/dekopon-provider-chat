@id("slack-history-on-slack")
permit(principal == Dekopon::Principal::"test-user",
       action == Dekopon::Action::"chat.slack.history.read", resource)
when { context has transport && context.transport == "slack.test" };

@id("discord-history-on-discord")
permit(principal == Dekopon::Principal::"test-user",
       action == Dekopon::Action::"chat.discord.history.read", resource)
when { context has transport && context.transport == "discord.test" };
