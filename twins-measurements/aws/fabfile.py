# Follow the instructions in `readme.md` to get started.
# Remember to call `source ~/bin/aws-mfa` every 24 hours.
import boto3
from botocore.exceptions import ClientError
from fabric import task, Connection, ThreadingGroup as Group
from paramiko import RSAKey
import os
import glob

ec2 = boto3.client('ec2')
region = os.environ.get("AWS_EC2_REGION")


# --- Start Config ---

def credentials():
    ''' Set the username and path to key file. '''
    return {
        'user': 'ubuntu',
        'keyfile': '/Users/asonnino/.ssh/aws-fb.pem'
    }


def filter(instance):
    ''' Specify a filter to select only the desired hosts. '''
    name = next(tag['Value'] for tag in instance.tags if 'Name'in tag['Key'])
    name = name.casefold()
    return ('twins'.casefold() in name) and ('generator' not in name)

# --- End Config ---


def set_hosts(ctx, status='running', cred=credentials, filter=filter):
    ''' Helper function to set the credentials and a list of instances into
    context; the instances are filtered with the filter provided as input.
    '''
    # Set credentials into the context
    credentials = cred()
    ctx.user = credentials['user']
    ctx.keyfile = credentials['keyfile']  # This is only used the task `info`
    ctx.connect_kwargs.pkey = RSAKey.from_private_key_file(
        credentials['keyfile'])

    # Get all instances for a given status.
    ec2resource = boto3.resource('ec2')
    instances = ec2resource.instances.filter(
        Filters=[{'Name': 'instance-state-name', 'Values': [status]}]
    )

    # Get all instances that match the input filter
    ctx.instances = [x for x in instances if filter(x)]
    ctx.hosts = [x.public_ip_address for x in instances if filter(x)]


@task
def test(ctx):
    ''' Test the connection with all hosts.
    If the command succeeds, it prints "Hello, World!" for each host.

    COMMANDS:	fab test
    '''
    set_hosts(ctx)
    g = Group(*ctx.hosts, user=ctx.user, connect_kwargs=ctx.connect_kwargs)
    g.run('echo "Hello, World!"')


@task
def info(ctx):
    ''' Print commands to ssh into hosts (debug).

    COMMANDS:	fab info
    '''
    set_hosts(ctx)
    print('\nAvailable machines:')
    for i, host in enumerate(ctx.hosts):
        print(f'{i}\t ssh -i {ctx.keyfile} {ctx.user}@{host}')
    print()


@task
def start(ctx):
    ''' Start all instances.

    COMMANDS:	fab start
    '''
    set_hosts(ctx, status='stopped')
    ids = [instance.id for instance in ctx.instances]
    if not ids:
        print('There are no instances to start.')
        return
    response = ec2.start_instances(InstanceIds=ids, DryRun=False)
    print(response['StartingInstances'])


@task
def stop(ctx):
    ''' Stop all instances.

    COMMANDS:	fab stop
    '''
    set_hosts(ctx, status='running')
    ids = [instance.id for instance in ctx.instances]
    if not ids:
        print('There are no instances to stop.')
        return
    response = ec2.stop_instances(InstanceIds=ids, DryRun=False)
    print(response)


@task
def install(ctx):
    ''' Cleanup and install twins on a fresh machine.

    COMMANDS:	fab install
    '''
    setup_script = 'twins-aws-setup.sh'
    restart_stalled_script = 'twins-aws-restart-stalled.sh'

    set_hosts(ctx)
    for host in ctx.hosts:
        c = Connection(host, user=ctx.user, connect_kwargs=ctx.connect_kwargs)
        c.put(setup_script, '.')
        c.run(f'chmod +x {setup_script}')

    # TODO: find a way to forgo the grub config prompt and run the setup
    # script automatically.
    print(f'The script "{setup_script}"" is now uploaded on every machine; '
          'Run it manually and pay attention to the APT grub config prompt.')


@task
def update(ctx):
    ''' Update the software from Github.

    COMMANDS:	fab update
    '''
    run_script = 'twins-aws-run.sh'
    maintenance_script = 'twins-aws-maintenance.sh'

    set_hosts(ctx)
    g = Group(*ctx.hosts, user=ctx.user, connect_kwargs=ctx.connect_kwargs)
    g.run('(cd libra/ && git pull)')

    for i, host in enumerate(ctx.hosts):
        c = Connection(host, user=ctx.user, connect_kwargs=ctx.connect_kwargs)
        c.put(run_script, '.')
        c.run(f'chmod +x {run_script}')
        c.put(maintenance_script, '.')
        c.run(f'chmod +x {maintenance_script}')
        c.run(f'echo -e "{i}\n{len(ctx.hosts)}" > config_file.txt')


@task
def upload(ctx):
    ''' [Obsolete] Upload testcases to all AWS servers.
    This command is obsolete; testcases are now generated straights on the
    AWS machines.

    COMMAND:    fab upload
    '''
    files = glob.glob('./testcases/*.bin')

    set_hosts(ctx)

    # Split testcases (fairly equally) amongst hosts.
    host_files = [[] for _ in ctx.hosts]
    for i, f in enumerate(files):
        host_files[i % len(ctx.hosts)].append(files[i])

    # Upload testcases to each host.
    for i, host in enumerate(ctx.hosts):
        c = Connection(host, user=ctx.user, connect_kwargs=ctx.connect_kwargs)
        c.run('mkdir -p testcases')
        progress = f'{i+1}/{len(ctx.hosts)}'
        print(f'[{progress}] Uploading {len(host_files[i])} files to {host}...')
        for file in host_files[i]:
            c.put(file, './testcases')
            c.local(f'mv {file} ./uploaded')


@task
def run(ctx):
    ''' Runs experiments with the specified configs.

    CONFIG:
        0: Run all testcases from the files located in /testcases.
        1: Run randomly selected scenarios.
        2: Generate testcases, sharded on each machine.

    COMMANDS:	fab run
    '''
    CONFIG = 0
    RUNS = 10  # Only used if CONFIG = 1.

    run_script = 'twins-aws-run.sh'
    maintenance_script = 'twins-aws-maintenance.sh'
    job = f'*/2 * * * * ./{maintenance_script}'

    # NOTE: Calling tmux in threaded groups does not work.
    set_hosts(ctx)
    for host in ctx.hosts:
        c = Connection(host, user=ctx.user, connect_kwargs=ctx.connect_kwargs)
        c.run(f'tmux new -d -s "twins" ./{run_script} {CONFIG} {RUNS}')
        if CONFIG == 0:
            c.run('crontab -r || true', hide=True)
            c.run(
                f'(crontab -l 2>/dev/null; echo "{job} -with args") | crontab -')


@task
def kill(ctx):
    ''' Kill the process on all machines and (optionally) clear all state.

    RESET:
        False: Only kill the process.
        True: Kill the process and delete all state and logs.

    COMMANDS:   fab kill
    '''
    RESET = False
    DELETE_ALL = False

    set_hosts(ctx)
    g = Group(*ctx.hosts, user=ctx.user, connect_kwargs=ctx.connect_kwargs)

    # Kill process.
    g.run(f'tmux kill-server || true')
    g.run('crontab -r || true')
    g.run('echo -1 > last_logfile')

    # Reset state and delete logs.
    if RESET:
        g.run(f'mv executed_tests/*.bin testcases/ || true')
        g.run(f'mv stalled_testcases/*.bin testcases/ || true')
        #g.run(f'rm stalled_testcases/*.log || true')
        g.run(f'rm logs/* || true')

    if DELETE_ALL:
        g.run(f'rm -r stalled_testcases || true')
        g.run(f'rm -r testcases || true')
        g.run(f'rm -r executed_tests || true')
        g.run(f'rm -r logs || true')
        g.run(f'rm generator_logs || true')


@task
def status(ctx):
    ''' Prints the execution progress.

    COMMANDS:	fab status
    '''
    set_hosts(ctx)

    status = {}
    def count_lines(dir): return f'ls -l {dir} | grep -v ^l | wc -l'
    print('Gathering data...\n')
    for host in ctx.hosts:
        c = Connection(host, user=ctx.user, connect_kwargs=ctx.connect_kwargs)
        remaining = c.run(count_lines('testcases'), hide=True)
        executed = c.run(count_lines('executed_tests'), hide=True)
        stalled = c.run(count_lines('stalled_testcases'), hide=True)
        try:
            # Both logs and testcases are stored in 'stalled_testcases'.
            stalled = int(stalled.stdout) // 2
            remaining = int(remaining.stdout)
            executed = int(executed.stdout) + stalled
            total = remaining + executed
        except ValueError:
            total = remaining = executed = stalled = 'N/A'

        status[host] = (executed, total, stalled)

    print('--------------------------------------------')
    print('HOST\t\tPROGRESS\tSTALLED')
    print('--------------------------------------------')
    for host, values in status.items():
        print(f'{host}\t{values[0]}/{values[1]}\t\t{values[2]}')
    print('--------------------------------------------')
    total_executed = sum([v[0] for v in status.values()])
    total_total = sum([v[1] for v in status.values()])
    total_stalled = sum([v[2] for v in status.values()])
    print(f'TOTAL:\t\t{total_executed}/{total_total}\t\t{total_stalled}')
    print('--------------------------------------------\n')

@task
def status2(ctx):
    ''' Prints the execution progress.

    COMMANDS:	fab status
    '''
    set_hosts(ctx)

    status = {}
    def count_lines(dir): return f'ls -l {dir} | grep -v ^l | wc -l'
    print('Gathering data...\n')
    for host in ctx.hosts:
        c = Connection(host, user=ctx.user, connect_kwargs=ctx.connect_kwargs)
        executed = c.run(count_lines('executed_tests'), hide=True)
        try:
            status[host] = int(executed.stdout)
        except ValueError:
            status[host] = 'N/A'

    print('--------------------------------------------')
    print('HOST\t\tEXECUTED')
    print('--------------------------------------------')
    for host, values in status.items():
        print(f'{host}\t{values}')
    print('--------------------------------------------')
    total = sum([v for v in status.values()])
    print(f'TOTAL:\t\t{total}')
    print('--------------------------------------------\n')
